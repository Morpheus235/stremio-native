//! Resource URLs: `{base}/{resource}/{type}/{id}[/{extra}].json`.
//!
//! Path segments and extra values are percent-encoded like JavaScript's `encodeURIComponent`
//! (everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )`), which is what addon SDKs decode. A raw
//! genre such as `Sport & News` or `Kids/Family` would otherwise split the extra string or the
//! path and the addon would answer for a genre that does not exist.

/// One `name=value` pair of the extra segment, in order.
pub type ExtraArg<'a> = (&'a str, &'a str);

pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The extra segment (`genre=Action&skip=100`), or `None` when no extra is set. Empty values are
/// dropped: an addon reads `genre=` as a genre named "".
pub fn extra_segment(extra: &[ExtraArg<'_>]) -> Option<String> {
    let parts: Vec<String> = extra
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v)))
        .collect();
    (!parts.is_empty()).then(|| parts.join("&"))
}

pub fn resource_url(base: &str, resource: &str, kind: &str, id: &str, extra: &[ExtraArg<'_>]) -> String {
    let mut url = format!(
        "{}/{}/{}/{}",
        base.trim_end_matches('/'),
        encode_component(resource),
        encode_component(kind),
        encode_component(id)
    );
    if let Some(extra) = extra_segment(extra) {
        url.push('/');
        url.push_str(&extra);
    }
    url.push_str(".json");
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_catalog_url() {
        assert_eq!(
            resource_url("https://v3-cinemeta.strem.io", "catalog", "movie", "top", &[]),
            "https://v3-cinemeta.strem.io/catalog/movie/top.json"
        );
    }

    #[test]
    fn extras_are_joined_and_encoded() {
        assert_eq!(
            resource_url("https://a.example/", "catalog", "movie", "top", &[("genre", "Sci-Fi"), ("skip", "100")]),
            "https://a.example/catalog/movie/top/genre=Sci-Fi&skip=100.json"
        );
        assert_eq!(
            resource_url("https://a.example", "catalog", "series", "top", &[("search", "the office")]),
            "https://a.example/catalog/series/top/search=the%20office.json"
        );
    }

    #[test]
    fn a_genre_with_reserved_characters_stays_one_value() {
        let url = resource_url("https://a.example", "catalog", "tv", "channels", &[("genre", "Sport & News")]);
        assert_eq!(url, "https://a.example/catalog/tv/channels/genre=Sport%20%26%20News.json");
        let url = resource_url("https://a.example", "catalog", "tv", "channels", &[("genre", "Kids/Family")]);
        assert_eq!(url, "https://a.example/catalog/tv/channels/genre=Kids%2FFamily.json");
        let url = resource_url("https://a.example", "catalog", "tv", "it", &[("genre", "Città")]);
        assert_eq!(url, "https://a.example/catalog/tv/it/genre=Citt%C3%A0.json");
    }

    #[test]
    fn ids_with_colons_are_encoded_and_empty_extras_dropped() {
        assert_eq!(
            resource_url("https://a.example", "stream", "series", "tt0903747:1:2", &[("genre", "")]),
            "https://a.example/stream/series/tt0903747%3A1%3A2.json"
        );
    }
}
