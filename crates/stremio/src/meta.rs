//! Which addons a meta request goes to, and which answer is used.
//!
//! stremio-core's MetaDetails asks EVERY addon that declares `meta` for the type and id prefix
//! (`AggrRequest::AllOfResource`) and shows the first one, in addon order, that answered with a
//! meta. Asking only the first serving addon breaks whenever that addon declares more than it
//! answers: MAL-Stremio declares `meta` for `mal`, `kitsu`, `tt`, `tvdb` and `tmdb` ids and answers
//! `{"meta":{}}` (or 403/404) for anything it cannot map, so placed above Cinemeta it emptied
//! every IMDb title's page. Trying the sources in order until one decodes gives core's choice.

use crate::manifest::Addon;
use crate::types::{Meta, MetaResponse};
use crate::url::resource_url;
use crate::{get_json, Fetch, FetchError};

/// The meta URL of every addon serving `meta` for `kind`/`id`, in addon order.
pub fn meta_urls(addons: &[Addon], kind: &str, id: &str) -> Vec<String> {
    addons
        .iter()
        .filter(|a| a.manifest.serves("meta", kind, id))
        .map(|a| resource_url(&a.base, "meta", kind, id, &[]))
        .collect()
}

/// The first meta `urls` yields, in order; when none does, the first source's failure. An empty
/// `urls` is a caller error and reads as a 404.
pub fn fetch_meta(fetch: &dyn Fetch, urls: &[String]) -> Result<Meta, FetchError> {
    let mut first_err = None;
    for url in urls {
        match get_json::<MetaResponse>(fetch, url, &[]) {
            Ok(r) => return Ok(r.meta),
            Err(FetchError::Cancelled) => return Err(FetchError::Cancelled),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or(FetchError::Status(404)))
}

/// The answer of a race over `urls` asked all at once (Stremio's own apps ask every source
/// together): `slots[i]` is source `i`'s result once it landed. The first source in ORDER that
/// succeeded wins as soon as every source before it has failed; `None` while that is not yet
/// decided. When all failed, the first source's failure.
pub fn pick_in_order(slots: &[Option<Result<Meta, FetchError>>]) -> Option<Result<Meta, FetchError>> {
    pick(slots, false)
}

/// [`pick_in_order`] once the earlier sources' grace ([`META_GRACE_MS`] after a later success) ran
/// out: the first success in order wins even while a source ahead of it is still asking.
pub fn pick_after_grace(slots: &[Option<Result<Meta, FetchError>>]) -> Option<Result<Meta, FetchError>> {
    pick(slots, true)
}

/// How long a later source's success waits for the sources ahead of it. A public TMDB addon
/// answering 504 after 12 s held every episode page that long behind AIOStreams' 0.3 s answer.
pub const META_GRACE_MS: u64 = 1000;

fn pick(slots: &[Option<Result<Meta, FetchError>>], grace_over: bool) -> Option<Result<Meta, FetchError>> {
    for slot in slots {
        match slot {
            None if grace_over => {}
            None => return None,
            Some(Ok(m)) => return Some(Ok(m.clone())),
            Some(Err(_)) => {}
        }
    }
    if slots.iter().any(Option::is_none) {
        return None; // nothing succeeded yet: keep waiting for whoever still asks
    }
    match slots.first() {
        Some(Some(Err(e))) => Some(Err(e.clone())),
        _ => Some(Err(FetchError::Status(404))),
    }
}

/// Decode a meta body the way [`fetch_meta`] does, for a racing caller with its own transport.
pub fn decode_meta(body: &[u8]) -> Result<Meta, FetchError> {
    serde_json::from_slice::<MetaResponse>(body).map(|r| r.meta).map_err(|e| FetchError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFetch;
    use crate::manifest::Manifest;

    /// After the grace, a later success wins over a source ahead that still asks; before it,
    /// it waits; with nothing succeeded, nothing is decided while anyone asks.
    #[test]
    fn after_the_grace_a_later_success_wins_over_a_slow_source_ahead() {
        let m = |name: &str| Meta { preview: crate::types::MetaPreview { name: name.into(), ..Default::default() }, ..Default::default() };
        let slots = vec![None, Some(Ok(m("aio")))];
        assert!(pick_in_order(&slots).is_none(), "within the grace the first source may still answer");
        assert_eq!(pick_after_grace(&slots).unwrap().unwrap().preview.name, "aio");
        let none_yet: Vec<Option<Result<Meta, FetchError>>> = vec![None, Some(Err(FetchError::Status(504)))];
        assert!(pick_after_grace(&none_yet).is_none(), "no success yet: keep waiting");
    }

    /// A slow first source no longer holds the page (13.1 s on the TV for an episode list a
    /// second source had at once): the race waits only for sources AHEAD of a success.
    #[test]
    fn the_race_takes_the_first_success_in_order_without_waiting_behind_it() {
        let m = |name: &str| Meta { preview: crate::types::MetaPreview { name: name.into(), ..Default::default() }, ..Default::default() };
        let err = || Err(FetchError::Status(500));
        assert_eq!(pick_in_order(&[None, Some(Ok(m("b")))]), None, "the first may still win");
        assert_eq!(pick_in_order(&[Some(err()), Some(Ok(m("b"))), None]).unwrap().unwrap().preview.name, "b");
        assert_eq!(pick_in_order(&[Some(Ok(m("a"))), None]).unwrap().unwrap().preview.name, "a", "no wait behind a win");
        assert_eq!(pick_in_order(&[Some(Err(FetchError::Status(404))), Some(err())]), Some(Err(FetchError::Status(404))));
    }

    /// The real MAL-Stremio manifest (trimmed) above Cinemeta, as a user may order them.
    fn addons() -> Vec<Addon> {
        let mal = Manifest::parse(include_bytes!("fixtures/mal_stremio_manifest.json")).unwrap();
        let cinemeta = Manifest::parse(
            br#"{"id":"com.linvo.cinemeta","resources":["catalog","meta"],"types":["movie","series"],"idPrefixes":["tt"]}"#,
        )
        .unwrap();
        vec![Addon::new("https://mal.example/u1/manifest.json", mal), Addon::new("https://v3-cinemeta.strem.io/manifest.json", cinemeta)]
    }

    #[test]
    fn an_addon_that_declares_but_cannot_answer_falls_through_to_the_next() {
        let urls = meta_urls(&addons(), "series", "tt0903747");
        assert_eq!(urls, ["https://mal.example/u1/meta/series/tt0903747.json", "https://v3-cinemeta.strem.io/meta/series/tt0903747.json"]);
        // MAL-Stremio's reply for an id it has no mapping for (app/routes/meta.py)
        let fetch = FakeFetch::default()
            .with(&urls[0], r#"{"meta":{}}"#)
            .with(&urls[1], include_str!("fixtures/cinemeta_breaking_bad.json"));
        let meta = fetch_meta(&fetch, &urls).unwrap();
        assert_eq!((meta.preview.name.as_str(), meta.videos.len()), ("Breaking Bad", 5));
    }

    #[test]
    fn the_first_answer_wins_and_the_first_failure_is_reported() {
        let urls = meta_urls(&addons(), "series", "tt0903747");
        let fetch = FakeFetch::default().with(&urls[0], include_str!("fixtures/cinemeta_breaking_bad.json"));
        assert!(fetch_meta(&fetch, &urls).is_ok());
        assert_eq!(fetch.asked.borrow().len(), 1, "a source that answered ends the walk");
        let fetch = FakeFetch::default().failing(&urls[0], FetchError::Status(403));
        assert_eq!(fetch_meta(&fetch, &urls), Err(FetchError::Status(403)));
    }
}
