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

fn parse<T: serde::de::DeserializeOwned>(raw: &[u8]) -> Result<T, ApiError> {
    let env: Envelope<T> = serde_json::from_slice(raw).map_err(|e| FetchError::Decode(e.to_string()))?;
    match (env.result, env.error) {
        (_, Some(err)) => Err(ApiError::Api { code: err.code, message: err.message }),
        (Some(result), None) => Ok(result),
        (None, None) => Err(FetchError::Decode("neither result nor error".into()).into()),
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
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

pub fn login(fetch: &dyn Fetch, email: &str, password: &str) -> Result<Session, ApiError> {
    call(fetch, "login", json!({"type": "Login", "email": email, "password": password, "facebook": false}))
}

pub fn logout(fetch: &dyn Fetch, auth_key: &str) -> Result<(), ApiError> {
    call::<serde_json::Value>(fetch, "logout", json!({"type": "Logout", "authKey": auth_key})).map(|_| ())
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
    let c: Collection = call(fetch, "addonCollectionGet", json!({"type": "AddonCollectionGet", "authKey": auth_key, "update": true}))?;
    Ok(addons_of(c))
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
}
