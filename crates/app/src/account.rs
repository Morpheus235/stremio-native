//! **The account's off-frame half**: the jobs of `sn_screens`' account, addons and library stores
//! run against the Stremio API, the platform's protected storage and the app's private files.
//!
//! ## Where the session is kept
//!
//! The auth key is the account's whole credential, so it is kept the way PlxNative keeps its Plex
//! session: in the storage helper's canonical DB8 object (one per flavour, the `object_id`s of
//! `ci/install-identities.json`), as the PROTECTED auth payload of a `ReplaceAuth` commit. The
//! helper seals it with the TV's key manager (`com.webos.service.keymanager3`), or, where that is
//! refused, keeps it in the DB8 object whose ACL only this app can read
//! (`ProtectionRequest::KeymanagerWithAclFallback`); the app never writes the key to a file.
//! Sign-out is a `ClearTenure` commit, which drops the payload and its key. [`HelperVault`] is
//! that; [`Vault`] is the seam the tests replace.
//!
//! The addon collection is NOT credential material but its transport URLs can carry the user's
//! configuration (a debrid key in a path), and it can exceed the canonical object's 256 KB logical
//! cap, so it is kept as an owner-only file in the app's private data directory ([`DataDir`]),
//! named by a hash of the account id and removed at sign-out. The search switches and the
//! first-run "continue without signing in" choice are files there too.
//!
//! Nothing here logs the key, a URL or an email address.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use sn_base::eventlog::log;
use sn_base::task::OffFrame;
use sn_screens::stores::account::{Account, AccountJob, AccountLanding, Secret, Stored};
use sn_screens::stores::addons::{AddonsJob, AddonsLanding};
use sn_screens::stores::library::{LibraryJob, LibraryLanding};
use sn_stremio::account as api;
use sn_stremio::search::AddonFilter;
use sn_stremio::Fetch;

use crate::fetch::{NetFetch, Profile};

// ---------------------------------------------------------------------------------------------
// The seams.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// No helper, a refused activation, a timeout.
    Unavailable,
    /// A session is stored but cannot be opened (its key is gone).
    Locked,
    /// Stored bytes that are not a session.
    Corrupt,
}

/// Where the session is kept.
pub trait Vault: Send + Sync {
    fn load(&self) -> Result<Option<Account>, VaultError>;
    fn save(&self, account: &Account) -> Result<(), VaultError>;
    fn clear(&self) -> Result<(), VaultError>;
}

/// The app's private files.
pub trait Files: Send + Sync {
    fn read(&self, name: &str) -> Option<Vec<u8>>;
    fn write(&self, name: &str, bytes: &[u8]) -> bool;
    fn remove(&self, name: &str);
}

/// An HTTP transport for one job, on the worker that runs it.
pub trait Net: Send + Sync {
    fn with(&self, off: &OffFrame, f: &mut dyn FnMut(&dyn Fetch));
}

/// `NetFetch` under the API profile.
pub struct ApiNet;

impl Net for ApiNet {
    fn with(&self, off: &OffFrame, f: &mut dyn FnMut(&dyn Fetch)) {
        f(&NetFetch::new(off, Profile::Api));
    }
}

/// Everything the account's jobs reach.
#[derive(Clone)]
pub struct Services {
    pub net: Arc<dyn Net>,
    pub vault: Arc<dyn Vault>,
    pub files: Arc<dyn Files>,
}

impl Services {
    /// The product's: the API over `sn_net`, the storage helper, the private data directory.
    pub fn device() -> Self {
        Services { net: Arc::new(ApiNet), vault: Arc::new(HelperVault), files: Arc::new(DataDir::default()) }
    }
}

// ---------------------------------------------------------------------------------------------
// The session's stored form.

fn session_json(a: &Account) -> String {
    serde_json::json!({"v": 1, "authKey": a.auth_key.expose(), "user": {"_id": a.user_id, "email": a.email}}).to_string()
}

fn parse_session(s: &str) -> Option<Account> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let key = v.get("authKey")?.as_str()?.trim();
    if key.is_empty() {
        return None;
    }
    let user = v.get("user");
    let field = |name: &str| user.and_then(|u| u.get(name)).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some(Account { user_id: field("_id"), email: field("email"), auth_key: Secret::new(key) })
}

/// The protected storage: the storage helper's canonical object (see the module doc).
pub struct HelperVault;

impl HelperVault {
    fn snapshot() -> Result<Option<sn_platform::storage::client::Snapshot>, VaultError> {
        use sn_platform::storage::client::{self, Load};
        match client::load() {
            Ok(Load::Missing) => Ok(None),
            Ok(Load::Present(s)) => Ok(Some(s)),
            Err(sn_platform::storage::client::ClientError::Corrupt) => Err(VaultError::Corrupt),
            Err(_) => Err(VaultError::Unavailable),
        }
    }

    fn commit(mutation: impl Fn(&sn_platform::storage::state::PublicPayload) -> sn_platform::storage::wire::WireMutation) -> Result<(), VaultError> {
        use sn_platform::storage::client;
        use sn_platform::storage::state::{Generation, PublicPayload};
        use sn_platform::storage::wire::{CommitStatus, ReconcileStatus, Response};
        // one retry: a Conflict means another owner (Settings, consent) wrote between our read
        // and our commit, and the next read sees it
        for _ in 0..2 {
            let snap = Self::snapshot()?;
            let public = snap.as_ref().map(|s| s.state.public.clone()).unwrap_or_else(PublicPayload::default);
            let expected = snap.as_ref().map(|s| (s.db_rev.clone(), s.state.expected()));
            let op = Generation::random().map_err(|_| VaultError::Unavailable)?;
            match client::commit(expected.as_ref().map(|(r, e)| (r.as_str(), *e)), op, mutation(&public)) {
                Ok(Response::Commit { status: CommitStatus::Committed, .. }) => return Ok(()),
                Ok(Response::Reconcile { status: ReconcileStatus::Applied, .. }) => return Ok(()),
                Ok(Response::Commit { status: CommitStatus::Conflict, .. }) => continue,
                Ok(_) | Err(_) => return Err(VaultError::Unavailable),
            }
        }
        Err(VaultError::Unavailable)
    }
}

impl Vault for HelperVault {
    fn load(&self) -> Result<Option<Account>, VaultError> {
        use sn_platform::storage::state::Status;
        use sn_platform::storage::wire::AuthLoad;
        let Some(snap) = Self::snapshot()? else { return Ok(None) };
        if snap.state.status == Status::Cleared {
            return Ok(None);
        }
        match snap.auth {
            AuthLoad::None => Ok(None),
            AuthLoad::Plaintext { payload } => parse_session(&payload.0).map(Some).ok_or(VaultError::Corrupt),
            AuthLoad::Locked { .. } => Err(VaultError::Locked),
        }
    }

    fn save(&self, account: &Account) -> Result<(), VaultError> {
        use sn_platform::storage::wire::{ProtectionRequest, SecretString, WireMutation};
        let payload = session_json(account);
        Self::commit(|public| WireMutation::ReplaceAuth {
            public: serde_json::to_value(public).unwrap_or(serde_json::Value::Null),
            payload: SecretString(payload.clone()),
            protection: ProtectionRequest::KeymanagerWithAclFallback,
        })
    }

    fn clear(&self) -> Result<(), VaultError> {
        use sn_platform::storage::state::Status;
        use sn_platform::storage::wire::WireMutation;
        match Self::snapshot()? {
            None => return Ok(()),
            Some(s) if s.state.status == Status::Cleared => return Ok(()),
            Some(_) => {}
        }
        Self::commit(|_| WireMutation::ClearTenure {})
    }
}

/// The app's private data directory: the first of `paths::private_data_candidates` that can be
/// made owner-only and written, chosen once.
#[derive(Default)]
pub struct DataDir {
    root: OnceLock<Option<PathBuf>>,
    /// A fixed root (tests).
    fixed: Option<PathBuf>,
}

impl DataDir {
    pub fn at(root: PathBuf) -> Self {
        DataDir { root: OnceLock::new(), fixed: Some(root) }
    }

    fn root(&self) -> Option<&Path> {
        self.root
            .get_or_init(|| {
                let candidates = match &self.fixed {
                    Some(r) => vec![r.clone()],
                    None => sn_base::paths::private_data_candidates(),
                };
                candidates.into_iter().find(|c| private_dir(c))
            })
            .as_deref()
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        let ok = !name.is_empty() && name.len() <= 96 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') && !name.starts_with('.');
        if !ok {
            return None;
        }
        Some(self.root()?.join(name))
    }
}

pub(crate) fn private_dir(path: &Path) -> bool {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let made = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path).is_ok();
    if !made {
        return false;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else { return false };
    if !meta.is_dir() {
        return false;
    }
    if meta.permissions().mode() & 0o077 != 0 && std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).is_err() {
        return false;
    }
    let probe = path.join(".probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

impl Files for DataDir {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.path(name)?;
        let bytes = std::fs::read(path).ok()?;
        (bytes.len() <= 4 << 20).then_some(bytes)
    }

    /// Atomic: a temporary beside it, owner-only, renamed over the old file.
    fn write(&self, name: &str, bytes: &[u8]) -> bool {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(path) = self.path(name) else { return false };
        let tmp = path.with_extension("tmp");
        let wrote = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all()))
            .is_ok();
        if !wrote || std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return false;
        }
        true
    }

    fn remove(&self, name: &str) {
        if let Some(path) = self.path(name) {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The private files' names and shapes.

/// "Continue without signing in" was chosen.
pub const SKIPPED_FILE: &str = "signin-skipped.json";
/// Search's per-addon switches.
pub const SEARCH_FILE: &str = "search-addons.json";
/// The Streams page's per-addon switches (addons not asked for streams).
pub const STREAMS_FILE: &str = "stream-addons.json";

/// The device copy of `user_id`'s addon collection: named by a hash, never by the id itself.
pub fn collection_file(user_id: &str) -> String {
    let digest = sn_base::sha256::sha256(format!("stremionative-collection\n{user_id}").as_bytes());
    let hex: String = digest.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("addons-{hex}.json")
}

pub fn filter_json(f: &AddonFilter) -> Vec<u8> {
    serde_json::json!({"v": 1, "disabled": f.disabled.iter().collect::<Vec<_>>()}).to_string().into_bytes()
}

pub fn parse_filter(bytes: &[u8]) -> AddonFilter {
    let v: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_default();
    AddonFilter {
        disabled: v
            .get("disabled")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------------------------
// The jobs.

/// One account job, on a worker.
pub fn run_account(off: &OffFrame, services: &Services, job: AccountJob) -> AccountLanding {
    let mut out = None;
    let mut job = Some(job);
    services.net.with(off, &mut |fetch| {
        if let Some(job) = job.take() {
            out = Some(run_account_with(fetch, &*services.vault, &*services.files, job));
        }
    });
    out.unwrap_or(AccountLanding::Forgotten)
}

/// [`run_account`] over any transport and stores (the tests' fakes).
pub fn run_account_with(fetch: &dyn Fetch, vault: &dyn Vault, files: &dyn Files, job: AccountJob) -> AccountLanding {
    match job {
        AccountJob::Restore => AccountLanding::Restored(match vault.load() {
            Ok(Some(account)) => {
                log("account: a stored session was restored");
                Stored::Session(account)
            }
            Ok(None) => Stored::None { skipped: files.read(SKIPPED_FILE).is_some() },
            Err(e) => {
                log(&format!("account: the stored session could not be read ({e:?})"));
                Stored::Unavailable
            }
        }),
        AccountJob::Login { email, password } => AccountLanding::LoggedIn(
            api::login(fetch, &email, password.expose()).map(|s| (Secret::new(s.auth_key), s.user)),
        ),
        AccountJob::LinkCreate => AccountLanding::LinkCreated(api::link_create(fetch)),
        AccountJob::LinkRead { code } => {
            let result = api::link_read(fetch, &code).map(Secret::new);
            AccountLanding::LinkRead { code, result }
        }
        AccountJob::Confirm { auth_key } => {
            AccountLanding::Confirmed(api::get_user(fetch, auth_key.expose()).map(|user| (auth_key, user)))
        }
        AccountJob::Save { account } => {
            let ok = vault.save(&account);
            if let Err(e) = ok {
                log(&format!("account: the session could not be kept ({e:?}); it lasts until the app closes"));
            }
            AccountLanding::Saved(ok.is_ok())
        }
        AccountJob::Forget { auth_key } => {
            if let Err(e) = vault.clear() {
                log(&format!("account: the stored session could not be removed ({e:?})"));
            }
            if let Some(key) = auth_key {
                // best effort: the device has already forgotten the key either way
                let _ = api::logout(fetch, key.expose());
            }
            files.write(SKIPPED_FILE, b"{\"v\":1}");
            AccountLanding::Forgotten
        }
        AccountJob::Skip => {
            files.write(SKIPPED_FILE, b"{\"v\":1}");
            AccountLanding::Skipped
        }
    }
}

/// One addons job, on a worker. `None` for a write, which lands nothing.
pub fn run_addons(off: &OffFrame, services: &Services, job: AddonsJob) -> Option<AddonsLanding> {
    let mut out = None;
    let mut job = Some(job);
    services.net.with(off, &mut |fetch| {
        if let Some(job) = job.take() {
            out = run_addons_with(fetch, &*services.files, job);
        }
    });
    out
}

pub fn run_addons_with(fetch: &dyn Fetch, files: &dyn Files, job: AddonsJob) -> Option<AddonsLanding> {
    match job {
        AddonsJob::ReadCache { user_id } => {
            let addons = files.read(&collection_file(&user_id)).and_then(|b| api::parse_collection_cache(&b));
            Some(AddonsLanding::CacheRead { user_id, addons })
        }
        AddonsJob::Fetch { user_id, auth_key } => {
            Some(AddonsLanding::Fetched { user_id, result: api::addon_collection_cached(fetch, auth_key.expose()) })
        }
        AddonsJob::WriteCache { user_id, bytes } => {
            if !files.write(&collection_file(&user_id), &bytes) {
                log("addons: the collection could not be kept on the device");
            }
            None
        }
        AddonsJob::DropCache { user_id } => {
            files.remove(&collection_file(&user_id));
            None
        }
        AddonsJob::ReadFilter => Some(AddonsLanding::FilterRead(files.read(SEARCH_FILE).map(|b| parse_filter(&b)).unwrap_or_default())),
        AddonsJob::WriteFilter(filter) => {
            files.write(SEARCH_FILE, &filter_json(&filter));
            None
        }
        AddonsJob::ReadStreamFilter => Some(AddonsLanding::StreamFilterRead(files.read(STREAMS_FILE).map(|b| parse_filter(&b)).unwrap_or_default())),
        AddonsJob::WriteStreamFilter(filter) => {
            files.write(STREAMS_FILE, &filter_json(&filter));
            None
        }
    }
}

/// One library job, on a worker.
pub fn run_library(off: &OffFrame, services: &Services, job: LibraryJob) -> LibraryLanding {
    let mut out = None;
    let mut job = Some(job);
    services.net.with(off, &mut |fetch| {
        if let Some(job) = job.take() {
            out = Some(run_library_with(fetch, job));
        }
    });
    out.expect("Net::with runs its closure")
}

pub fn run_library_with(fetch: &dyn Fetch, job: LibraryJob) -> LibraryLanding {
    match job {
        LibraryJob::Fetch { user_id, auth_key } => {
            LibraryLanding::Fetched { user_id, result: sn_stremio::library::fetch_library(fetch, auth_key.expose()) }
        }
        LibraryJob::Add { user_id, auth_key, entry } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let result = sn_stremio::library::add_to_library(fetch, auth_key.expose(), &entry, &sn_stremio::library::rfc3339_ms(now));
            LibraryLanding::Added { user_id, id: entry.id, result }
        }
        LibraryJob::Edit { user_id, auth_key, id, edit } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let result = sn_stremio::library::edit_library(fetch, auth_key.expose(), &id, edit, &sn_stremio::library::rfc3339_ms(now));
            LibraryLanding::Edited { user_id, id, result }
        }
        LibraryJob::Mark { user_id, auth_key, entry, edit } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let result = sn_stremio::library::mark_library(fetch, auth_key.expose(), &entry, edit, &sn_stremio::library::rfc3339_ms(now));
            LibraryLanding::Edited { user_id, id: entry.id, result }
        }
        LibraryJob::Progress { user_id, auth_key, progress, last } => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            let result = sn_stremio::library::put_progress(fetch, auth_key.expose(), &progress, &sn_stremio::library::rfc3339_ms(now));
            LibraryLanding::Progressed { user_id, last, result }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sn_stremio::FetchError;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// The session store as a map: what the helper would hold.
    #[derive(Default)]
    pub struct MemVault {
        pub saved: Mutex<Option<String>>,
        pub unavailable: bool,
    }
    impl Vault for MemVault {
        fn load(&self) -> Result<Option<Account>, VaultError> {
            if self.unavailable {
                return Err(VaultError::Unavailable);
            }
            Ok(self.saved.lock().unwrap().as_deref().and_then(parse_session))
        }
        fn save(&self, account: &Account) -> Result<(), VaultError> {
            if self.unavailable {
                return Err(VaultError::Unavailable);
            }
            *self.saved.lock().unwrap() = Some(session_json(account));
            Ok(())
        }
        fn clear(&self) -> Result<(), VaultError> {
            *self.saved.lock().unwrap() = None;
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct MemFiles(pub Mutex<HashMap<String, Vec<u8>>>);
    impl Files for MemFiles {
        fn read(&self, name: &str) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(name).cloned()
        }
        fn write(&self, name: &str, bytes: &[u8]) -> bool {
            self.0.lock().unwrap().insert(name.into(), bytes.to_vec());
            true
        }
        fn remove(&self, name: &str) {
            self.0.lock().unwrap().remove(name);
        }
    }

    /// The API as a URL → body map; every request is recorded (URL and body).
    #[derive(Default)]
    pub struct FakeApi {
        pub bodies: Mutex<HashMap<String, Result<String, FetchError>>>,
        pub seen: Mutex<Vec<(String, String)>>,
    }
    impl FakeApi {
        pub fn answer(&self, method: &str, body: &str) {
            self.bodies.lock().unwrap().insert(format!("{}/api/{method}", api::API), Ok(body.into()));
        }
        pub fn fail(&self, method: &str, e: FetchError) {
            self.bodies.lock().unwrap().insert(format!("{}/api/{method}", api::API), Err(e));
        }
        pub fn answer_url(&self, url: &str, body: &str) {
            self.bodies.lock().unwrap().insert(url.into(), Ok(body.into()));
        }
    }
    impl Fetch for FakeApi {
        fn get(&self, url: &str, _: &[(String, String)]) -> Result<Vec<u8>, FetchError> {
            self.seen.lock().unwrap().push((url.into(), String::new()));
            match self.bodies.lock().unwrap().get(url) {
                Some(Ok(b)) => Ok(b.clone().into_bytes()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(FetchError::Status(404)),
            }
        }
        fn post(&self, url: &str, _: &[(String, String)], body: &[u8]) -> Result<Vec<u8>, FetchError> {
            self.seen.lock().unwrap().push((url.into(), String::from_utf8_lossy(body).into()));
            match self.bodies.lock().unwrap().get(url) {
                Some(Ok(b)) => Ok(b.clone().into_bytes()),
                Some(Err(e)) => Err(e.clone()),
                None => Err(FetchError::Status(404)),
            }
        }
    }
    impl Net for FakeApi {
        fn with(&self, _off: &OffFrame, f: &mut dyn FnMut(&dyn Fetch)) {
            f(self);
        }
    }

    fn acct() -> Account {
        Account { user_id: "u1".into(), email: "a@b.c".into(), auth_key: Secret::new("k-secret") }
    }

    #[test]
    fn a_saved_session_restores_and_a_forgotten_one_does_not() {
        let vault = MemVault::default();
        let files = MemFiles::default();
        let api = FakeApi::default();
        api.answer("logout", r#"{"result":{}}"#);
        assert!(matches!(run_account_with(&api, &vault, &files, AccountJob::Save { account: acct() }), AccountLanding::Saved(true)));
        match run_account_with(&api, &vault, &files, AccountJob::Restore) {
            AccountLanding::Restored(Stored::Session(a)) => assert_eq!(a, acct()),
            other => panic!("{other:?}"),
        }
        run_account_with(&api, &vault, &files, AccountJob::Forget { auth_key: Some(Secret::new("k-secret")) });
        assert!(matches!(run_account_with(&api, &vault, &files, AccountJob::Restore), AccountLanding::Restored(Stored::None { skipped: true })));
        let seen = api.seen.lock().unwrap().clone();
        assert!(seen.iter().any(|(u, b)| u.ends_with("/api/logout") && b.contains("\"Logout\"")), "the API session is ended too");
    }

    #[test]
    fn an_unreadable_store_restores_as_unavailable_and_a_failed_save_says_so() {
        let vault = MemVault { unavailable: true, ..Default::default() };
        let files = MemFiles::default();
        let api = FakeApi::default();
        assert!(matches!(run_account_with(&api, &vault, &files, AccountJob::Restore), AccountLanding::Restored(Stored::Unavailable)));
        assert!(matches!(run_account_with(&api, &vault, &files, AccountJob::Save { account: acct() }), AccountLanding::Saved(false)));
    }

    #[test]
    fn login_sends_the_login_request_and_carries_the_key_back_as_a_secret() {
        let api = FakeApi::default();
        api.answer("login", r#"{"result":{"authKey":"k1","user":{"_id":"u1","email":"a@b.c"}}}"#);
        let landing = run_account_with(&api, &MemVault::default(), &MemFiles::default(), AccountJob::Login { email: "a@b.c".into(), password: Secret::new("pw") });
        let AccountLanding::LoggedIn(Ok((key, user))) = landing else { panic!() };
        assert_eq!((key.expose(), user.id.as_str()), ("k1", "u1"));
        let (url, body) = api.seen.lock().unwrap()[0].clone();
        assert!(url.ends_with("/api/login") && body.contains(r#""type":"Login""#) && body.contains(r#""email":"a@b.c""#));
    }

    #[test]
    fn the_collection_cache_is_per_account_and_the_filter_round_trips() {
        let files = MemFiles::default();
        assert_ne!(collection_file("u1"), collection_file("u2"));
        assert!(!collection_file("u1").contains("u1"), "the file is not named by the account");
        let api = FakeApi::default();
        api.answer("addonCollectionGet", r#"{"result":{"addons":[{"transportUrl":"https://a.example/manifest.json","manifest":{"id":"a","name":"A"}}]}}"#);
        let Some(AddonsLanding::Fetched { result: Ok((addons, bytes)), .. }) =
            run_addons_with(&api, &files, AddonsJob::Fetch { user_id: "u1".into(), auth_key: Secret::new("k") })
        else {
            panic!()
        };
        run_addons_with(&api, &files, AddonsJob::WriteCache { user_id: "u1".into(), bytes });
        let Some(AddonsLanding::CacheRead { addons: Some(cached), .. }) = run_addons_with(&api, &files, AddonsJob::ReadCache { user_id: "u1".into() }) else {
            panic!()
        };
        assert_eq!(cached, addons);
        assert!(matches!(run_addons_with(&api, &files, AddonsJob::ReadCache { user_id: "u2".into() }), Some(AddonsLanding::CacheRead { addons: None, .. })));
        run_addons_with(&api, &files, AddonsJob::DropCache { user_id: "u1".into() });
        assert!(files.read(&collection_file("u1")).is_none());
        let mut f = AddonFilter::default();
        f.toggle("com.linvo.cinemeta");
        run_addons_with(&api, &files, AddonsJob::WriteFilter(f.clone()));
        assert!(matches!(run_addons_with(&api, &files, AddonsJob::ReadFilter), Some(AddonsLanding::FilterRead(g)) if g == f));
    }

    #[test]
    fn the_data_dir_writes_owner_only_files_atomically() {
        use std::os::unix::fs::PermissionsExt;
        let root = sn_base::testscratch::process_dir("acct-datadir");
        let dir = DataDir::at(root.join("data"));
        assert!(dir.write("x.json", b"one"));
        assert!(dir.write("x.json", b"two"));
        assert_eq!(dir.read("x.json").as_deref(), Some(&b"two"[..]));
        let mode = std::fs::metadata(root.join("data").join("x.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "owner-only: {mode:o}");
        let dmode = std::fs::metadata(root.join("data")).unwrap().permissions().mode();
        assert_eq!(dmode & 0o077, 0, "owner-only dir: {dmode:o}");
        assert!(!dir.write("../escape", b"x"), "a name is a file in the directory");
        dir.remove("x.json");
        assert!(dir.read("x.json").is_none());
    }

    #[test]
    fn the_stored_session_is_never_logged_by_the_landings() {
        let vault = MemVault::default();
        let _ = vault.save(&acct());
        let landing = run_account_with(&FakeApi::default(), &vault, &MemFiles::default(), AccountJob::Restore);
        assert!(!format!("{landing:?}").contains("k-secret"));
    }
}
