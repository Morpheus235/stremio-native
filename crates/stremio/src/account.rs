//! The Stremio account API: sign in, and read the addon collection the user installed.
//!
//! Every call is `POST {API}/api/{method}` with a JSON body tagged by `type`; the answer is
//! `{"result": ...}` or `{"error": {"message", "code"}}`. Signed out, the app runs on
//! [`DEFAULT_ADDONS`].

use crate::manifest::{Addon, Manifest};
use crate::{Fetch, FetchError};
use serde::Deserialize;
use serde_json::json;

pub const API: &str = "https://api.strem.io";

/// What a signed-out install uses.
pub const DEFAULT_ADDONS: &[&str] = &["https://v3-cinemeta.strem.io/manifest.json"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    Fetch(FetchError),
    /// The API answered with an error object (wrong password, expired session...).
    Api { code: i64, message: String },
}

impl From<FetchError> for ApiError {
    fn from(e: FetchError) -> Self {
        ApiError::Fetch(e)
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: Option<T>,
    error: Option<ApiErrorBody>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

fn call<T: serde::de::DeserializeOwned>(fetch: &dyn Fetch, method: &str, body: serde_json::Value) -> Result<T, ApiError> {
    let raw = fetch.post(&format!("{API}/api/{method}"), &[("Content-Type".into(), "application/json".into())], body.to_string().as_bytes())?;
    parse(&raw)
}

pub(crate) fn parse<T: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<T, ApiError> {
    let env: Envelope<T> = serde_json::from_slice(raw).map_err(|e| FetchError::Decode(e.to_string()))?;
    match (env.result, env.error) {
        (_, Some(err)) => Err(ApiError::Api { code: err.code, message: err.message }),
        (Some(result), None) => Ok(result),
        (None, None) => Err(FetchError::Decode("neither result nor error".into()).into()),
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub auth_key: String,
    pub user: User,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct User {
    #[serde(rename = "_id", default)]
    pub id: String,
    #[serde(default)]
    pub email: String,
}

impl std::fmt::Display for ApiError {
    /// What a read-out may say: the API's own message (it is written for people — "Wrong
    /// passphrase"), or the transport's closed vocabulary. Never a URL, never a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Api { message, .. } if !message.trim().is_empty() => f.write_str(message.trim()),
            ApiError::Api { code, .. } => write!(f, "error {code}"),
            ApiError::Fetch(e) => write!(f, "{e}"),
        }
    }
}

impl ApiError {
    /// The request never got an answer from the API (DNS, connect, TLS, timeout, a 5xx): worth a
    /// retry, unlike an answer that said no.
    pub fn is_network(&self) -> bool {
        match self {
            ApiError::Fetch(FetchError::Transport(_) | FetchError::Cancelled) => true,
            ApiError::Fetch(FetchError::Status(s)) => *s >= 500 || *s == 429 || *s == 408,
            ApiError::Fetch(FetchError::Decode(_)) | ApiError::Api { .. } => false,
        }
    }
}

impl std::fmt::Debug for Session {
    /// Never the auth key: it is the account's whole credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("auth_key", &"<redacted>").field("user", &self.user).finish()
    }
}

pub fn login(fetch: &dyn Fetch, email: &str, password: &str) -> Result<Session, ApiError> {
    call(fetch, "login", json!({"type": "Login", "email": email, "password": password, "facebook": false}))
}

pub fn logout(fetch: &dyn Fetch, auth_key: &str) -> Result<(), ApiError> {
    call::<serde_json::Value>(fetch, "logout", json!({"type": "Logout", "authKey": auth_key})).map(|_| ())
}

/// The account an auth key belongs to (`getUser`): what a key obtained some other way (a device
/// link) is confirmed with before it is kept.
pub fn get_user(fetch: &dyn Fetch, auth_key: &str) -> Result<User, ApiError> {
    call(fetch, "getUser", json!({"type": "GetUser", "authKey": auth_key}))
}

/// The device-link service: a short code the viewer confirms on another device that is signed in
/// (`stremio-core` `constants::LINK_API_URL`, `types::api::LinkRequest`, `VERSION = "v2"`).
pub const LINK_API: &str = "https://link.stremio.com";

/// A link code to show: `link` is the page the viewer opens (the QR's payload), `code` what they
/// type there.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct LinkCode {
    pub code: String,
    pub link: String,
    /// The service's own QR rendering (unused: the app encodes `link` itself).
    #[serde(default)]
    pub qrcode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkData {
    auth_key: String,
}

/// `GET {LINK_API}/api/v2/create?type=Create` — `LinkRequest::Create` as `fetch_api` builds it
/// (endpoint joined with `api/`, then the versioned path, the request serialized as the query).
pub fn link_create(fetch: &dyn Fetch) -> Result<LinkCode, ApiError> {
    let raw = fetch.get(&format!("{LINK_API}/api/v2/create?type=Create"), &[])?;
    parse(&raw)
}

/// `GET {LINK_API}/api/v2/read?type=Read&code=…` — the auth key once the viewer confirmed the
/// code. Until then the service answers an error object, which is reported as such: the caller
/// polls again while the code is fresh.
pub fn link_read(fetch: &dyn Fetch, code: &str) -> Result<String, ApiError> {
    let raw = fetch.get(&format!("{LINK_API}/api/v2/read?type=Read&code={}", crate::url::encode_component(code)), &[])?;
    let data: LinkData = parse(&raw)?;
    if data.auth_key.trim().is_empty() {
        return Err(FetchError::Decode("empty auth key".into()).into());
    }
    Ok(data.auth_key)
}

#[derive(Deserialize)]
struct Collection {
    #[serde(default)]
    addons: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    transport_url: String,
    manifest: Manifest,
}

/// The user's installed addons, in their order. A descriptor that does not decode is skipped,
/// never allowed to empty the collection.
pub fn addon_collection(fetch: &dyn Fetch, auth_key: &str) -> Result<Vec<Addon>, ApiError> {
    addon_collection_cached(fetch, auth_key).map(|(addons, _)| addons)
}

/// [`addon_collection`] plus the bytes to keep on the device, so the next boot shows the same
/// addons before the account answers: `{"addons":[…]}` with only the descriptors that decoded,
/// each exactly as the API sent it. [`parse_collection_cache`] reads it back.
pub fn addon_collection_cached(fetch: &dyn Fetch, auth_key: &str) -> Result<(Vec<Addon>, Vec<u8>), ApiError> {
    let c: Collection = call(fetch, "addonCollectionGet", json!({"type": "AddonCollectionGet", "authKey": auth_key, "update": true}))?;
    let kept: Vec<serde_json::Value> = c
        .addons
        .into_iter()
        .filter(|v| match serde_json::from_value::<Descriptor>(v.clone()) {
            Ok(_) => true,
            Err(e) => {
                // the name and the decoder's reason only: the transport URL can carry the
                // addon's configuration (keys), which never goes to the log
                let name = v.pointer("/manifest/name").and_then(|n| n.as_str()).unwrap_or("?");
                sn_base::eventlog::log(&format!("addons: '{name}' skipped — its manifest does not decode: {e}"));
                false
            }
        })
        .collect();
    let cache = json!({ "addons": kept }).to_string().into_bytes();
    Ok((addons_of(Collection { addons: kept }), cache))
}

/// A collection kept by [`addon_collection_cached`]. `None` for bytes that are not one.
pub fn parse_collection_cache(bytes: &[u8]) -> Option<Vec<Addon>> {
    serde_json::from_slice::<Collection>(bytes).ok().map(addons_of)
}

fn addons_of(c: Collection) -> Vec<Addon> {
    c.addons
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Descriptor>(v).ok())
        .map(|d| Addon::new(&d.transport_url, d.manifest))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_error_is_reported_not_swallowed() {
        let e = parse::<Session>(br#"{"error":{"message":"Wrong passphrase","code":3}}"#).unwrap_err();
        assert_eq!(e, ApiError::Api { code: 3, message: "Wrong passphrase".into() });
    }

    #[test]
    fn a_login_result_carries_the_auth_key() {
        let s: Session = parse(br#"{"result":{"authKey":"k123","user":{"_id":"u1","email":"a@b.c"}}}"#).unwrap();
        assert_eq!(s.auth_key, "k123");
        assert_eq!(s.user.id, "u1");
    }

    #[test]
    fn the_collection_keeps_order_and_skips_broken_descriptors() {
        let c: Collection = parse(
            br#"{"result":{"addons":[
                {"transportUrl":"https://v3-cinemeta.strem.io/manifest.json","manifest":{"id":"com.linvo.cinemeta","name":"Cinemeta"}},
                {"transportUrl":"https://broken.example/manifest.json"},
                {"transportUrl":"https://live.example/cfg/manifest.json","manifest":{"id":"livetv","name":"Live TV"},"flags":{"official":false}}]}}"#,
        )
        .unwrap();
        let addons = addons_of(c);
        let bases: Vec<_> = addons.iter().map(|a| a.base.as_str()).collect();
        assert_eq!(bases, ["https://v3-cinemeta.strem.io", "https://live.example/cfg"]);
    }

    use crate::fake::FakeFetch;

    #[test]
    fn the_cached_collection_reads_back_as_the_same_addons_in_the_same_order() {
        let f = FakeFetch::default().with(
            &format!("{API}/api/addonCollectionGet"),
            r#"{"result":{"addons":[
                {"transportUrl":"https://live.example/cfg/manifest.json","manifest":{"id":"livetv","name":"Live TV"}},
                {"transportUrl":"https://broken.example/manifest.json"},
                {"transportUrl":"https://v3-cinemeta.strem.io/manifest.json","manifest":{"id":"com.linvo.cinemeta","name":"Cinemeta"}}]}}"#,
        );
        let (addons, cache) = addon_collection_cached(&f, "k").unwrap();
        assert_eq!(addons.len(), 2);
        assert_eq!(parse_collection_cache(&cache), Some(addons), "the device copy is the same list");
        assert!(!String::from_utf8_lossy(&cache).contains("broken.example"), "a descriptor that did not decode is not kept");
        assert_eq!(parse_collection_cache(b"not json"), None);
    }

    #[test]
    fn the_device_link_asks_the_link_service_and_reads_the_key_once_confirmed() {
        let create = format!("{LINK_API}/api/v2/create?type=Create");
        let read = format!("{LINK_API}/api/v2/read?type=Read&code=AB12");
        let f = FakeFetch::default()
            .with(&create, r#"{"result":{"code":"AB12","link":"https://link.stremio.com/AB12","qrcode":"data"}}"#)
            .with(&read, r#"{"error":{"message":"Not linked yet","code":1}}"#);
        let code = link_create(&f).unwrap();
        assert_eq!((code.code.as_str(), code.link.as_str()), ("AB12", "https://link.stremio.com/AB12"));
        assert!(matches!(link_read(&f, "AB12"), Err(ApiError::Api { .. })), "not linked yet is an answer, not a key");
        let f = FakeFetch::default().with(&read, r#"{"result":{"authKey":"secret-key"}}"#);
        assert_eq!(link_read(&f, "AB12").unwrap(), "secret-key");
    }

    #[test]
    fn a_session_never_prints_its_key_and_errors_read_as_people_wrote_them() {
        let s: Session = parse(br#"{"result":{"authKey":"k-secret","user":{"_id":"u1","email":"a@b.c"}}}"#).unwrap();
        assert!(!format!("{s:?}").contains("k-secret"));
        assert_eq!(ApiError::Api { code: 3, message: "Wrong passphrase".into() }.to_string(), "Wrong passphrase");
        assert!(ApiError::Fetch(FetchError::Transport("timed out".into())).is_network());
        assert!(ApiError::Fetch(FetchError::Status(503)).is_network());
        assert!(!ApiError::Api { code: 3, message: String::new() }.is_network());
    }
}
