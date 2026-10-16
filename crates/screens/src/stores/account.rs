//! **The account store** — whether this television is signed in to a Stremio account, and the two
//! ways it gets there: email + password, or the device link (a code confirmed on a phone).
//!
//! Pure like the other stores: commands in, [`AccountJob`]s out, [`AccountLanding`]s in. The
//! application runs the jobs on workers: the API calls over `sn_stremio::account`, the session
//! read and write through the platform's protected storage (`app::account`). This store never
//! touches either, and it never lets the auth key out except into a job:
//!
//! - **The key is a [`Secret`].** Its `Debug` prints `<redacted>`; the only way to read it is
//!   [`Secret::expose`], which the job runner calls at the request.
//! - **The session's life**: `Restoring` (the boot's read of the stored session) → `SignedIn` or
//!   `SignedOut`; a sign-in lands as `SignedIn` and asks for it to be SAVED; a sign-out goes to
//!   `SignedOut` at once and asks for the stored session to be REMOVED (and the API's session
//!   ended, best effort).
//! - **A sign-in attempt's read-out** ([`SignIn`]): idle, working, the API's refusal in its own
//!   words (wrong password), or a network failure the viewer can retry with the same credentials
//!   (held in memory only for that retry, and dropped on any answer).
//! - **The device link** ([`Link`]): a code is created, shown, and read until the account behind
//!   it answers; a key read that way is CONFIRMED (`getUser`) before it is kept. A code is
//!   abandoned after [`LINK_TTL_MS`] and the screen offers a new one.

use sn_stremio::account::{ApiError, LinkCode, User};

/// A credential: printed as `<redacted>`, compared and cloned like a string.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    /// The value, for the one place that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A signed-in account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub user_id: String,
    /// User text (shown on the Addons page); never logged.
    pub email: String,
    pub auth_key: Secret,
}

/// What the stored session read back as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    Session(Account),
    /// No session stored. `skipped`: the viewer chose to go on without signing in.
    None { skipped: bool },
    /// The protected storage could not be read (no helper, a locked key). Not "signed out" on the
    /// device's record, but nothing to sign in with this run.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    Restoring,
    SignedOut {
        /// The viewer has chosen to browse without an account (the first-run sign-in is not
        /// offered again).
        skipped: bool,
    },
    SignedIn(Account),
}

/// The email + password attempt's read-out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    Idle,
    Working,
    /// The API said no; its message, as it wrote it ("Wrong passphrase").
    Rejected(String),
    /// No answer from the API; the reason, and a retry is offered.
    Offline(String),
}

/// How long a device-link code is read before it is abandoned.
pub const LINK_TTL_MS: u32 = 5 * 60 * 1000;
/// How often a shown code is read.
pub const LINK_POLL_MS: u32 = 3_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Off,
    Creating,
    /// Shown; `since_ms` is the tick it was shown at.
    Waiting { code: LinkCode, since_ms: u32 },
    /// The key it produced is being confirmed.
    Confirming,
    Failed(String),
    Expired,
}

/// Work the application runs off the frame thread.
#[derive(Clone, Debug)]
pub enum AccountJob {
    /// Read the stored session (and the "continue without signing in" choice).
    Restore,
    Login { email: String, password: Secret },
    LinkCreate,
    LinkRead { code: String },
    /// Confirm a key the link produced (`getUser`).
    Confirm { auth_key: Secret },
    /// Keep `account` in protected storage.
    Save { account: Account },
    /// Remove the stored session; end `auth_key`'s API session (best effort).
    Forget { auth_key: Option<Secret> },
    /// Remember that the viewer chose to go on without an account.
    Skip,
}

#[derive(Clone, Debug)]
pub enum AccountLanding {
    Restored(Stored),
    LoggedIn(Result<(Secret, User), ApiError>),
    LinkCreated(Result<LinkCode, ApiError>),
    LinkRead { code: String, result: Result<Secret, ApiError> },
    Confirmed(Result<(Secret, User), ApiError>),
    /// The save finished; `false` when the session could not be kept (it still works this run).
    Saved(bool),
    Forgotten,
    Skipped,
}

/// What a screen may ask of the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountCmd {
    SignIn { email: String, password: Secret },
    /// The network failure's retry: the same credentials again.
    Retry,
    StartLink,
    /// A tick while a code is shown: read it if it is time.
    PollLink { now_ms: u32 },
    CancelLink,
    ContinueSignedOut,
    SignOut,
}

pub struct AccountStore {
    session: SessionState,
    sign_in: SignIn,
    /// The last attempt's credentials, kept only while its network failure is on screen.
    retry: Option<(String, Secret)>,
    link: Link,
    link_reading: bool,
    link_last_read: u32,
    /// The stored copy could not be written (the session works until the app closes).
    save_failed: bool,
    gen: u32,
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountStore {
    /// Restoring: the boot's first job is [`AccountJob::Restore`] ([`Self::boot`]).
    pub fn new() -> Self {
        AccountStore {
            session: SessionState::Restoring,
            sign_in: SignIn::Idle,
            retry: None,
            link: Link::Off,
            link_reading: false,
            link_last_read: 0,
            save_failed: false,
            gen: 0,
        }
    }

    /// A store already signed out (tests, and a boot that skips the restore).
    pub fn signed_out(skipped: bool) -> Self {
        AccountStore { session: SessionState::SignedOut { skipped }, ..Self::new() }
    }

    pub fn boot(&mut self) -> Vec<AccountJob> {
        vec![AccountJob::Restore]
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn session(&self) -> &SessionState {
        &self.session
    }

    pub fn account(&self) -> Option<&Account> {
        match &self.session {
            SessionState::SignedIn(a) => Some(a),
            _ => None,
        }
    }

    pub fn sign_in(&self) -> &SignIn {
        &self.sign_in
    }

    pub fn link(&self) -> &Link {
        &self.link
    }

    pub fn save_failed(&self) -> bool {
        self.save_failed
    }

    /// Should the sign-in screen be offered on its own (a first run, nobody chose yet)?
    pub fn wants_first_run_sign_in(&self) -> bool {
        matches!(self.session, SessionState::SignedOut { skipped: false })
    }

    pub fn command(&mut self, cmd: AccountCmd) -> Vec<AccountJob> {
        match cmd {
            AccountCmd::SignIn { email, password } => {
                let email = email.trim().to_string();
                if email.is_empty() || password.is_empty() || self.sign_in == SignIn::Working {
                    return Vec::new();
                }
                self.sign_in = SignIn::Working;
                self.retry = Some((email.clone(), password.clone()));
                self.bump();
                vec![AccountJob::Login { email, password }]
            }
            AccountCmd::Retry => match (&self.sign_in, self.retry.clone()) {
                (SignIn::Offline(_), Some((email, password))) => {
                    self.sign_in = SignIn::Working;
                    self.bump();
                    vec![AccountJob::Login { email, password }]
                }
                _ => Vec::new(),
            },
            AccountCmd::StartLink => {
                if matches!(self.link, Link::Creating | Link::Confirming) {
                    return Vec::new();
                }
                self.link = Link::Creating;
                self.link_reading = false;
                self.bump();
                vec![AccountJob::LinkCreate]
            }
            AccountCmd::PollLink { now_ms } => {
                let Link::Waiting { code, since_ms } = &self.link else { return Vec::new() };
                if now_ms.wrapping_sub(*since_ms) >= LINK_TTL_MS {
                    self.link = Link::Expired;
                    self.link_reading = false;
                    self.bump();
                    return Vec::new();
                }
                if self.link_reading || now_ms.wrapping_sub(self.link_last_read) < LINK_POLL_MS {
                    return Vec::new();
                }
                self.link_reading = true;
                self.link_last_read = now_ms;
                vec![AccountJob::LinkRead { code: code.code.clone() }]
            }
            AccountCmd::CancelLink => {
                if self.link != Link::Off {
                    self.link = Link::Off;
                    self.link_reading = false;
                    self.bump();
                }
                Vec::new()
            }
            AccountCmd::ContinueSignedOut => {
                if matches!(self.session, SessionState::SignedIn(_)) {
                    return Vec::new();
                }
                self.session = SessionState::SignedOut { skipped: true };
                self.bump();
                vec![AccountJob::Skip]
            }
            AccountCmd::SignOut => {
                let key = match std::mem::replace(&mut self.session, SessionState::SignedOut { skipped: true }) {
                    SessionState::SignedIn(a) => Some(a.auth_key),
                    _ => None,
                };
                self.sign_in = SignIn::Idle;
                self.retry = None;
                self.link = Link::Off;
                self.save_failed = false;
                self.bump();
                vec![AccountJob::Forget { auth_key: key }]
            }
        }
    }

    /// The tick a link code landed on: the screen's clock, for its expiry.
    pub fn land(&mut self, landing: AccountLanding, now_ms: u32) -> Vec<AccountJob> {
        let mut jobs = Vec::new();
        match landing {
            AccountLanding::Restored(stored) => {
                if self.session != SessionState::Restoring {
                    return jobs; // a sign-in finished first: it wins
                }
                self.session = match stored {
                    Stored::Session(a) => SessionState::SignedIn(a),
                    Stored::None { skipped } => SessionState::SignedOut { skipped },
                    // Nothing to sign in with, but do not push a first-run question at somebody
                    // whose session the device simply could not read this time.
                    Stored::Unavailable => SessionState::SignedOut { skipped: true },
                };
            }
            AccountLanding::LoggedIn(result) => {
                if self.sign_in != SignIn::Working {
                    return jobs;
                }
                match result {
                    Ok((key, user)) => {
                        self.retry = None;
                        self.sign_in = SignIn::Idle;
                        jobs.push(self.signed_in(key, user));
                    }
                    Err(e) if e.is_network() => self.sign_in = SignIn::Offline(e.to_string()),
                    Err(e) => {
                        self.retry = None;
                        self.sign_in = SignIn::Rejected(e.to_string());
                    }
                }
            }
            AccountLanding::LinkCreated(result) => {
                if self.link != Link::Creating {
                    return jobs;
                }
                self.link = match result {
                    Ok(code) => Link::Waiting { code, since_ms: now_ms },
                    Err(e) => Link::Failed(e.to_string()),
                };
                self.link_last_read = now_ms;
            }
            AccountLanding::LinkRead { code, result } => {
                self.link_reading = false;
                let Link::Waiting { code: shown, .. } = &self.link else { return jobs };
                if shown.code != code {
                    return jobs;
                }
                match result {
                    Ok(key) => {
                        self.link = Link::Confirming;
                        jobs.push(AccountJob::Confirm { auth_key: key });
                    }
                    // not confirmed yet (or a dropped read): the next poll asks again
                    Err(_) => return jobs,
                }
            }
            AccountLanding::Confirmed(result) => {
                if self.link != Link::Confirming {
                    return jobs;
                }
                match result {
                    Ok((key, user)) => {
                        self.link = Link::Off;
                        jobs.push(self.signed_in(key, user));
                    }
                    Err(e) => self.link = Link::Failed(e.to_string()),
                }
            }
            AccountLanding::Saved(ok) => {
                if self.save_failed == ok {
                    self.save_failed = !ok;
                } else {
                    return jobs;
                }
            }
            AccountLanding::Forgotten | AccountLanding::Skipped => return jobs,
        }
        self.bump();
        jobs
    }

    fn signed_in(&mut self, key: Secret, user: User) -> AccountJob {
        let account = Account { user_id: user.id, email: user.email, auth_key: key };
        self.session = SessionState::SignedIn(account.clone());
        self.save_failed = false;
        AccountJob::Save { account }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::FetchError;

    fn user() -> User {
        serde_json::from_str(r#"{"_id":"u1","email":"a@b.c"}"#).unwrap()
    }

    fn signing_in() -> AccountStore {
        let mut s = AccountStore::signed_out(false);
        let jobs = s.command(AccountCmd::SignIn { email: " a@b.c ".into(), password: Secret::new("pw") });
        assert!(matches!(&jobs[..], [AccountJob::Login { email, .. }] if email == "a@b.c"));
        assert_eq!(s.sign_in(), &SignIn::Working);
        s
    }

    #[test]
    fn a_sign_in_lands_signed_in_and_asks_for_the_session_to_be_kept() {
        let mut s = signing_in();
        assert!(s.command(AccountCmd::SignIn { email: "a@b.c".into(), password: Secret::new("pw") }).is_empty(), "one attempt at a time");
        let jobs = s.land(AccountLanding::LoggedIn(Ok((Secret::new("key"), user()))), 0);
        let [AccountJob::Save { account }] = &jobs[..] else { panic!("{jobs:?}") };
        assert_eq!(account.auth_key.expose(), "key");
        assert_eq!(s.account().map(|a| a.user_id.as_str()), Some("u1"));
        assert_eq!(s.sign_in(), &SignIn::Idle);
        assert!(!format!("{jobs:?} {:?}", s.session()).contains("key\""), "the key never prints");
        assert!(format!("{:?}", account.auth_key) == "<redacted>");
    }

    #[test]
    fn wrong_credentials_read_the_apis_message_and_offer_no_retry() {
        let mut s = signing_in();
        s.land(AccountLanding::LoggedIn(Err(ApiError::Api { code: 3, message: "Wrong passphrase".into() })), 0);
        assert_eq!(s.sign_in(), &SignIn::Rejected("Wrong passphrase".into()));
        assert!(s.command(AccountCmd::Retry).is_empty(), "a refusal is not retried with the same password");
        assert!(matches!(s.session(), SessionState::SignedOut { .. }));
    }

    #[test]
    fn a_network_failure_offers_a_retry_with_the_same_credentials() {
        let mut s = signing_in();
        s.land(AccountLanding::LoggedIn(Err(ApiError::Fetch(FetchError::Transport("timed out".into())))), 0);
        assert_eq!(s.sign_in(), &SignIn::Offline("transport: timed out".into()));
        let jobs = s.command(AccountCmd::Retry);
        assert!(matches!(&jobs[..], [AccountJob::Login { email, password }] if email == "a@b.c" && password.expose() == "pw"));
        assert_eq!(s.sign_in(), &SignIn::Working);
    }

    #[test]
    fn the_restore_decides_the_first_run_and_a_sign_in_that_finished_first_wins() {
        let mut s = AccountStore::new();
        assert_eq!(s.boot().len(), 1);
        s.land(AccountLanding::Restored(Stored::None { skipped: false }), 0);
        assert!(s.wants_first_run_sign_in());
        let mut s = AccountStore::new();
        s.land(AccountLanding::Restored(Stored::Unavailable), 0);
        assert!(!s.wants_first_run_sign_in(), "an unreadable store does not ask the first-run question");
        let acct = Account { user_id: "u1".into(), email: "a@b.c".into(), auth_key: Secret::new("k") };
        let mut s = AccountStore::new();
        s.land(AccountLanding::Restored(Stored::Session(acct.clone())), 0);
        assert_eq!(s.account(), Some(&acct));
    }

    #[test]
    fn sign_out_drops_the_session_and_asks_for_it_to_be_forgotten() {
        let acct = Account { user_id: "u1".into(), email: "a@b.c".into(), auth_key: Secret::new("k") };
        let mut s = AccountStore::new();
        s.land(AccountLanding::Restored(Stored::Session(acct)), 0);
        let jobs = s.command(AccountCmd::SignOut);
        assert!(matches!(&jobs[..], [AccountJob::Forget { auth_key: Some(k) }] if k.expose() == "k"));
        assert_eq!(s.session(), &SessionState::SignedOut { skipped: true });
        assert!(!s.wants_first_run_sign_in(), "signing out is not a first run");
    }

    #[test]
    fn the_device_link_polls_confirms_and_expires() {
        let mut s = AccountStore::signed_out(false);
        assert!(matches!(&s.command(AccountCmd::StartLink)[..], [AccountJob::LinkCreate]));
        let code = LinkCode { code: "AB12".into(), link: "https://link.stremio.com/AB12".into(), qrcode: String::new() };
        s.land(AccountLanding::LinkCreated(Ok(code.clone())), 1000);
        assert!(matches!(s.link(), Link::Waiting { .. }));
        assert!(s.command(AccountCmd::PollLink { now_ms: 2000 }).is_empty(), "not before the poll interval");
        let jobs = s.command(AccountCmd::PollLink { now_ms: 1000 + LINK_POLL_MS });
        assert!(matches!(&jobs[..], [AccountJob::LinkRead { code }] if code == "AB12"));
        assert!(s.command(AccountCmd::PollLink { now_ms: 1000 + 3 * LINK_POLL_MS }).is_empty(), "one read out at a time");
        s.land(AccountLanding::LinkRead { code: "AB12".into(), result: Err(ApiError::Api { code: 1, message: "pending".into() }) }, 0);
        let jobs = s.command(AccountCmd::PollLink { now_ms: 1000 + 3 * LINK_POLL_MS });
        assert_eq!(jobs.len(), 1, "polls again after an unconfirmed read");
        let jobs = s.land(AccountLanding::LinkRead { code: "AB12".into(), result: Ok(Secret::new("lk")) }, 0);
        assert!(matches!(&jobs[..], [AccountJob::Confirm { auth_key }] if auth_key.expose() == "lk"));
        let jobs = s.land(AccountLanding::Confirmed(Ok((Secret::new("lk"), user()))), 0);
        assert!(matches!(&jobs[..], [AccountJob::Save { .. }]));
        assert!(s.account().is_some());
        // a fresh code that nobody confirms expires
        let mut s = AccountStore::signed_out(false);
        s.command(AccountCmd::StartLink);
        s.land(AccountLanding::LinkCreated(Ok(code)), 0);
        s.command(AccountCmd::PollLink { now_ms: LINK_TTL_MS });
        assert_eq!(s.link(), &Link::Expired);
    }

    #[test]
    fn continuing_without_an_account_is_remembered() {
        let mut s = AccountStore::signed_out(false);
        assert!(matches!(&s.command(AccountCmd::ContinueSignedOut)[..], [AccountJob::Skip]));
        assert!(!s.wants_first_run_sign_in());
    }
}
