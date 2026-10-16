//! **The library store** — the signed-in account's library (`sn_stremio::library`), which Home's
//! Continue Watching row and the Library page read.
//!
//! Pure: [`LibraryStore::sign_in`] plans the fetch, the landing folds in. Signed out there is no
//! library and nothing is asked. What a resume OPENS is not decided here but by
//! `LibraryItem::resume_target` (the episode, never the bare series id), read by the screen that
//! was pressed.
//!
//! An item menu's *Add to library* is [`LibraryStore::add`]: one write per item at a time, and the
//! library is asked again once the account has it ([`AddState`] is what the menu reads).

use sn_stremio::account::ApiError;
use sn_stremio::library::{LibraryItem, NewEntry};

use super::account::Secret;

#[derive(Clone, Debug, PartialEq)]
pub enum LibraryState {
    /// Signed out: there is no library.
    None,
    Loading,
    Ready(Vec<LibraryItem>),
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum LibraryJob {
    Fetch { user_id: String, auth_key: Secret },
    /// Write `entry` into the account's library (`sn_stremio::library::add_to_library`).
    Add { user_id: String, auth_key: Secret, entry: NewEntry },
}

#[derive(Clone, Debug)]
pub enum LibraryLanding {
    Fetched { user_id: String, result: Result<Vec<LibraryItem>, ApiError> },
    Added { user_id: String, id: String, result: Result<(), ApiError> },
}

/// Where an item stands for the item menu's *Add to library*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddState {
    /// Not in the library: it can be added.
    Absent,
    /// The write is on its way.
    Adding,
    /// The last write failed: it can be tried again.
    Failed,
    /// The account's library holds it.
    In,
}

/// The types the Library page groups under their own heading, in this order; anything else
/// follows in the order it first appears.
pub const TYPE_ORDER: &[&str] = &["movie", "series", "tv", "channel"];

pub struct LibraryStore {
    state: LibraryState,
    user: Option<(String, Secret)>,
    /// Items whose write is on its way.
    adding: Vec<String>,
    /// Items whose last write failed.
    failed: Vec<String>,
    gen: u32,
}

impl Default for LibraryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryStore {
    pub fn new() -> Self {
        LibraryStore { state: LibraryState::None, user: None, adding: Vec::new(), failed: Vec::new(), gen: 0 }
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    pub fn state(&self) -> &LibraryState {
        &self.state
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
    }

    pub fn sign_in(&mut self, user_id: &str, auth_key: &Secret) -> Vec<LibraryJob> {
        if self.user.as_ref().is_some_and(|(u, _)| u == user_id) {
            return Vec::new();
        }
        self.user = Some((user_id.to_string(), auth_key.clone()));
        self.reload()
    }

    /// Ask again (a retry, or a return to Home after playing).
    pub fn reload(&mut self) -> Vec<LibraryJob> {
        let Some((user_id, key)) = self.user.clone() else { return Vec::new() };
        if self.state == LibraryState::Loading {
            return Vec::new();
        }
        if !matches!(self.state, LibraryState::Ready(_)) {
            self.state = LibraryState::Loading;
            self.bump();
        }
        vec![LibraryJob::Fetch { user_id, auth_key: key }]
    }

    /// Add `entry` to the account's library. Nothing while signed out, while it is already in
    /// it, or while its write is on its way.
    pub fn add(&mut self, entry: NewEntry) -> Vec<LibraryJob> {
        let Some((user_id, auth_key)) = self.user.clone() else { return Vec::new() };
        if matches!(self.add_state(&entry.id), AddState::In | AddState::Adding) {
            return Vec::new();
        }
        self.failed.retain(|i| *i != entry.id);
        self.adding.push(entry.id.clone());
        self.bump();
        vec![LibraryJob::Add { user_id, auth_key, entry }]
    }

    /// Where `id` stands for *Add to library*.
    pub fn add_state(&self, id: &str) -> AddState {
        if self.adding.iter().any(|i| i == id) {
            AddState::Adding
        } else if self.items().iter().any(|i| i.id == id && !i.removed && !i.temp) {
            AddState::In
        } else if self.failed.iter().any(|i| i == id) {
            AddState::Failed
        } else {
            AddState::Absent
        }
    }

    pub fn sign_out(&mut self) {
        self.user = None;
        self.adding.clear();
        self.failed.clear();
        if self.state != LibraryState::None {
            self.state = LibraryState::None;
            self.bump();
        }
    }

    /// Fold a landing in: whether the store changed, and what to ask next (a successful add asks
    /// for the library again, so the item shows where it was added).
    pub fn land(&mut self, landing: LibraryLanding) -> (bool, Vec<LibraryJob>) {
        let (user_id, result) = match landing {
            LibraryLanding::Fetched { user_id, result } => (user_id, result),
            LibraryLanding::Added { user_id, id, result } => {
                if !self.user.as_ref().is_some_and(|(u, _)| *u == user_id) {
                    return (false, Vec::new());
                }
                self.adding.retain(|i| *i != id);
                self.bump();
                return match result {
                    Ok(()) => {
                        // the reload is not skipped by one already on its way: that one may
                        // predate the write
                        let (user_id, auth_key) = self.user.clone().expect("checked above");
                        (true, vec![LibraryJob::Fetch { user_id, auth_key }])
                    }
                    Err(_) => {
                        self.failed.push(id);
                        (true, Vec::new())
                    }
                };
            }
        };
        if !self.user.as_ref().is_some_and(|(u, _)| *u == user_id) {
            return (false, Vec::new());
        }
        self.state = match result {
            Ok(items) => LibraryState::Ready(items),
            // a refresh that fails keeps what is shown
            Err(_) if matches!(self.state, LibraryState::Ready(_)) => return (false, Vec::new()),
            Err(e) => LibraryState::Failed(e.to_string()),
        };
        self.bump();
        (true, Vec::new())
    }

    pub fn items(&self) -> &[LibraryItem] {
        match &self.state {
            LibraryState::Ready(items) => items,
            _ => &[],
        }
    }

    /// Continue Watching: in progress, most recently played first.
    pub fn continue_watching(&self) -> Vec<&LibraryItem> {
        let mut cw: Vec<&LibraryItem> = self.items().iter().filter(|i| i.is_continue_watching()).collect();
        cw.sort_by(|a, b| b.recency().cmp(a.recency()));
        cw
    }

    /// The Library page's groups: every entry the account keeps (not removed, not temporary), by
    /// type ([`TYPE_ORDER`] first), most recently changed first within a type.
    pub fn by_type(&self) -> Vec<(String, Vec<&LibraryItem>)> {
        let mut groups: Vec<(String, Vec<&LibraryItem>)> = Vec::new();
        for item in self.items().iter().filter(|i| !i.removed && !i.temp) {
            match groups.iter_mut().find(|(k, _)| *k == item.kind) {
                Some((_, v)) => v.push(item),
                None => groups.push((item.kind.clone(), vec![item])),
            }
        }
        let rank = |k: &str| TYPE_ORDER.iter().position(|t| *t == k).unwrap_or(TYPE_ORDER.len());
        groups.sort_by_key(|(k, _)| rank(k));
        for (_, v) in &mut groups {
            v.sort_by(|a, b| b.recency().cmp(a.recency()));
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: &str) -> LibraryItem {
        serde_json::from_str(json).unwrap()
    }

    fn loaded() -> LibraryStore {
        let mut s = LibraryStore::new();
        assert_eq!(s.sign_in("u1", &Secret::new("k")).len(), 1);
        assert_eq!(s.state(), &LibraryState::Loading);
        let items = vec![
            item(r#"{"_id":"tt1","type":"movie","name":"Old","state":{"timeOffset":5,"lastWatched":"2026-01-01T00:00:00Z"}}"#),
            item(r#"{"_id":"tt2","type":"series","name":"Show","state":{"timeOffset":7,"video_id":"tt2:1:2","lastWatched":"2026-03-01T00:00:00Z"}}"#),
            item(r#"{"_id":"tt3","type":"movie","name":"Unstarted","_mtime":"2026-02-01T00:00:00Z"}"#),
            item(r#"{"_id":"ch1","type":"tv","name":"Channel"}"#),
            item(r#"{"_id":"tmp","type":"movie","name":"Temp","temp":true,"state":{"timeOffset":9}}"#),
        ];
        assert!(s.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(items) }).0);
        s
    }

    #[test]
    fn continue_watching_is_in_progress_items_most_recent_first() {
        let s = loaded();
        let ids: Vec<&str> = s.continue_watching().iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["tt2", "tt1"], "a temporary entry is not continued");
    }

    #[test]
    fn the_library_groups_by_type_in_the_shared_order() {
        let s = loaded();
        let groups: Vec<(String, Vec<&str>)> =
            s.by_type().into_iter().map(|(k, v)| (k, v.into_iter().map(|i| i.id.as_str()).collect())).collect();
        assert_eq!(
            groups,
            vec![("movie".into(), vec!["tt3", "tt1"]), ("series".into(), vec!["tt2"]), ("tv".into(), vec!["ch1"])]
        );
    }

    #[test]
    fn signed_out_there_is_no_library_and_a_late_landing_is_dropped() {
        let mut s = loaded();
        s.sign_out();
        assert_eq!(s.state(), &LibraryState::None);
        assert!(!s.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(Vec::new()) }).0);
        assert!(s.reload().is_empty());
        assert!(s.add(new("tt9")).is_empty(), "nothing is written signed out");
    }

    fn new(id: &str) -> NewEntry {
        NewEntry { id: id.into(), kind: "movie".into(), name: "N".into(), poster: None }
    }

    #[test]
    fn an_add_is_written_once_then_the_library_is_asked_again() {
        let mut s = loaded();
        assert_eq!(s.add_state("tt3"), AddState::In);
        assert!(s.add(new("tt3")).is_empty(), "already in the library");
        assert_eq!(s.add_state("tmp"), AddState::Absent, "a temporary entry is not in the library");
        let jobs = s.add(new("tt9"));
        assert!(matches!(&jobs[..], [LibraryJob::Add { entry, .. }] if entry.id == "tt9"));
        assert_eq!(s.add_state("tt9"), AddState::Adding);
        assert!(s.add(new("tt9")).is_empty(), "one write at a time");
        let (changed, jobs) = s.land(LibraryLanding::Added { user_id: "u1".into(), id: "tt9".into(), result: Ok(()) });
        assert!(changed);
        assert!(matches!(&jobs[..], [LibraryJob::Fetch { .. }]), "the library is asked again");
    }

    #[test]
    fn a_failed_add_says_so_and_can_be_tried_again() {
        let mut s = loaded();
        let _ = s.add(new("tt9"));
        let err = ApiError::Api { code: 1, message: "no".into() };
        let (changed, jobs) = s.land(LibraryLanding::Added { user_id: "u1".into(), id: "tt9".into(), result: Err(err) });
        assert!(changed && jobs.is_empty());
        assert_eq!(s.add_state("tt9"), AddState::Failed);
        assert_eq!(s.add(new("tt9")).len(), 1, "tried again");
        assert_eq!(s.add_state("tt9"), AddState::Adding);
    }
}
