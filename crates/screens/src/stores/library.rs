//! **The library store** — the signed-in account's library (`sn_stremio::library`), which Home's
//! Continue Watching row and the Library page read.
//!
//! Pure: [`LibraryStore::sign_in`] plans the fetch, the landing folds in. Signed out there is no
//! library and nothing is asked. What a resume OPENS is not decided here but by
//! `LibraryItem::resume_target` (the episode, never the bare series id), read by the screen that
//! was pressed.
//!
//! The item menu's and the Detail page's *Add to library* / *Remove from library* are
//! [`LibraryStore::add`] / [`LibraryStore::remove`], and *Remove from Continue Watching* is
//! [`LibraryStore::rewind`] (stremio-core's `RewindLibraryItem`). Each is OPTIMISTIC: the held
//! library changes at once (so the Library page, Continue Watching and every menu read the new
//! state on the next frame), one write per item is on its way at a time, a library fetch that
//! lands meanwhile has the change applied again on top, and a refused write puts the entry back
//! as it was and is queued for the application's toast ([`LibraryStore::take_failed`]). A write
//! that lands asks for the library again. [`AddState`] is what the menu and the button read.
//!
//! *Mark as watched / unwatched* (a movie, an episode, a season: [`LibraryStore::mark`]) is the
//! same optimistic write: the held entry's play state changes at once by stremio-core's rules
//! (`sn_stremio::library::PlayState`), a title the library lacks is held as core's temporary
//! entry, and a refusal rolls it back with its own toast ([`Refused::Mark`]).
//!
//! The player records where the viewer is with [`LibraryStore::progress`] (stremio-core's
//! `TimeChanged`, `sn_stremio::library::put_progress`) while it plays and when it stops; the held
//! entry moves at once by the same rules (the watched tick shows as the threshold is crossed), and
//! the last write of a playback asks for the library again, so Continue Watching shows where it
//! stopped.

use sn_stremio::account::ApiError;
use sn_stremio::library::{Edit, LibraryItem, NewEntry, PlayState, Progress, Sort};

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
    /// Change the account's entry `id` (`sn_stremio::library::edit_library`).
    Edit { user_id: String, auth_key: Secret, id: String, edit: Edit },
    /// Mark `entry` watched or unwatched (`sn_stremio::library::mark_library`; `edit` is a mark).
    Mark { user_id: String, auth_key: Secret, entry: NewEntry, edit: Edit },
    /// Record where a playback stands (`sn_stremio::library::put_progress`). `last`: the
    /// playback's final write, after which the library is asked again.
    Progress { user_id: String, auth_key: Secret, progress: Progress, last: bool },
}

#[derive(Clone, Debug)]
pub enum LibraryLanding {
    Fetched { user_id: String, result: Result<Vec<LibraryItem>, ApiError> },
    Added { user_id: String, id: String, result: Result<(), ApiError> },
    Edited { user_id: String, id: String, result: Result<(), ApiError> },
    Progressed { user_id: String, last: bool, result: Result<(), ApiError> },
}

/// Where an item stands for *Add to library* / *Remove from library*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddState {
    /// Not in the library: it can be added.
    Absent,
    /// Added here, the write on its way (the held library already has it).
    Adding,
    /// Removed here, the write on its way (the held library already dropped it).
    Removing,
    /// The account's library holds it: it can be removed.
    In,
}

/// A write the account refused, which the application tells the viewer about (and which the
/// store has already rolled back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Add,
    Remove,
    Rewind,
    /// A *Mark as watched / unwatched*.
    Mark,
}

#[derive(Clone, Debug)]
enum Op {
    Add(NewEntry),
    Edit(Edit),
    /// A watched mark on `entry`'s item, as of `now` (RFC 3339).
    Mark { entry: NewEntry, edit: Edit, now: String },
}

/// A write on its way: what it changed, and the entry as it was before (`None`: it was not held).
#[derive(Clone, Debug)]
struct Pending {
    id: String,
    op: Op,
    before: Option<LibraryItem>,
}

/// Apply `op` to the held `items`.
fn apply(items: &mut Vec<LibraryItem>, id: &str, op: &Op) {
    let held = items.iter_mut().find(|i| i.id == id);
    match (op, held) {
        (Op::Add(_), Some(i)) => {
            i.removed = false;
            i.temp = false;
        }
        (Op::Add(e), None) => items.push(LibraryItem {
            id: e.id.clone(),
            name: e.name.clone(),
            kind: e.kind.clone(),
            poster: e.poster.clone(),
            ..LibraryItem::default()
        }),
        (Op::Edit(Edit::Remove), Some(i)) => {
            i.removed = true;
            i.temp = false;
        }
        (Op::Edit(Edit::Rewind), Some(i)) => i.state.time_offset = 0,
        (Op::Edit(_), _) => {}
        (Op::Mark { edit, now, .. }, Some(i)) => {
            edit.apply_to(&mut i.state, now);
        }
        // core's temporary entry for a title the library lacks
        (Op::Mark { entry, edit, now }, None) if edit.creates() => {
            let mut state = PlayState { last_watched: Some(now.clone()), ..PlayState::default() };
            edit.apply_to(&mut state, now);
            items.push(LibraryItem {
                id: entry.id.clone(),
                name: entry.name.clone(),
                kind: entry.kind.clone(),
                poster: entry.poster.clone(),
                removed: true,
                temp: true,
                state,
                ..LibraryItem::default()
            });
        }
        (Op::Mark { .. }, None) => {}
    }
}

/// The types the Library page groups under their own heading, in this order; anything else
/// follows in the order it first appears.
pub const TYPE_ORDER: &[&str] = &["movie", "series", "tv", "channel"];

pub struct LibraryStore {
    state: LibraryState,
    user: Option<(String, Secret)>,
    /// The writes on their way, at most one per item.
    pending: Vec<Pending>,
    /// Refused writes the application has not told the viewer about yet.
    refused: Vec<Refused>,
    /// [`LibraryStore::recent_channels`], rebuilt whenever the store moves.
    recent: Vec<sn_stremio::types::MetaPreview>,
    gen: u32,
}

/// How many channels Live TV's Recently watched lists.
pub const RECENT_CHANNELS: usize = 20;

impl Default for LibraryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LibraryStore {
    pub fn new() -> Self {
        LibraryStore { state: LibraryState::None, user: None, pending: Vec::new(), refused: Vec::new(), recent: Vec::new(), gen: 0 }
    }

    pub fn gen(&self) -> u32 {
        self.gen
    }

    pub fn state(&self) -> &LibraryState {
        &self.state
    }

    fn bump(&mut self) {
        self.gen = self.gen.wrapping_add(1);
        let mut played: Vec<&LibraryItem> =
            self.items().iter().filter(|i| matches!(i.kind.as_str(), "tv" | "channel") && i.state.last_watched.as_deref().is_some_and(|w| !w.is_empty())).collect();
        played.sort_by(|a, b| b.recency().cmp(a.recency()));
        self.recent = played
            .into_iter()
            .take(RECENT_CHANNELS)
            .map(|i| sn_stremio::types::MetaPreview { id: i.id.clone(), kind: i.kind.clone(), name: i.name.clone(), poster: i.poster.clone(), ..Default::default() })
            .collect();
    }

    /// Live TV's Recently watched: the channels the account played, most recent first (at most
    /// [`RECENT_CHANNELS`]), as catalog items. A channel's play leaves no position, so Continue
    /// Watching never lists it (stremio-core's rule); its last play is kept all the same, and
    /// this reads only that — nothing is written for it.
    pub fn recent_channels(&self) -> &[sn_stremio::types::MetaPreview] {
        &self.recent
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
    /// it, or while a write for it is on its way.
    pub fn add(&mut self, entry: NewEntry) -> Vec<LibraryJob> {
        if self.add_state(&entry.id) != AddState::Absent {
            return Vec::new();
        }
        let id = entry.id.clone();
        self.begin(&id, Op::Add(entry.clone()), |user_id, auth_key| LibraryJob::Add { user_id, auth_key, entry })
    }

    /// Remove `id` from the account's library (stremio-core's `RemoveFromLibrary`). Nothing
    /// unless the library holds it and no write for it is on its way.
    pub fn remove(&mut self, id: &str) -> Vec<LibraryJob> {
        if self.add_state(id) != AddState::In {
            return Vec::new();
        }
        self.edit(id, Edit::Remove)
    }

    /// Drop `id` from Continue Watching (stremio-core's `RewindLibraryItem`). Nothing unless it
    /// is continued and no write for it is on its way.
    pub fn rewind(&mut self, id: &str) -> Vec<LibraryJob> {
        if !self.continuing(id) || self.pending.iter().any(|p| p.id == id) {
            return Vec::new();
        }
        self.edit(id, Edit::Rewind)
    }

    /// Is a write for `id` on its way (a mark waits for it)?
    pub fn writing(&self, id: &str) -> bool {
        self.pending.iter().any(|p| p.id == id)
    }

    /// The entry the account holds for `id` (temporary ones too: they carry play and watched state).
    pub fn item(&self, id: &str) -> Option<&LibraryItem> {
        self.items().iter().find(|i| i.id == id)
    }

    /// Is `id` in Continue Watching (as the held library stands)?
    pub fn continuing(&self, id: &str) -> bool {
        self.items().iter().any(|i| i.id == id && i.is_continue_watching())
    }

    /// The refused writes since the last call, oldest first (the application's toast).
    pub fn take_failed(&mut self) -> Vec<Refused> {
        std::mem::take(&mut self.refused)
    }

    fn edit(&mut self, id: &str, edit: Edit) -> Vec<LibraryJob> {
        let owned = id.to_string();
        self.begin(id, Op::Edit(edit.clone()), |user_id, auth_key| LibraryJob::Edit { user_id, auth_key, id: owned, edit })
    }

    /// *Mark as watched / unwatched* `entry`'s item (`edit` a watched mark, stremio-core's
    /// `MarkAsWatched` / `MarkVideoAsWatched` / `MarkSeasonAsWatched`), as of `now` (RFC 3339).
    /// Optimistic like the rest; a title the library lacks is held as core's temporary entry.
    /// Nothing while signed out, while a write for the item is on its way, or for an edit that is
    /// not a mark.
    pub fn mark(&mut self, entry: NewEntry, edit: Edit, now: &str) -> Vec<LibraryJob> {
        if !matches!(edit, Edit::MarkWatched(_) | Edit::MarkVideos(_)) {
            return Vec::new();
        }
        let id = entry.id.clone();
        let op = Op::Mark { entry: entry.clone(), edit: edit.clone(), now: now.to_string() };
        self.begin(&id, op, |user_id, auth_key| LibraryJob::Mark { user_id, auth_key, entry, edit })
    }

    /// Change the held library now and plan the write.
    fn begin(&mut self, id: &str, op: Op, job: impl FnOnce(String, Secret) -> LibraryJob) -> Vec<LibraryJob> {
        let Some((user_id, auth_key)) = self.user.clone() else { return Vec::new() };
        if self.pending.iter().any(|p| p.id == id) {
            return Vec::new();
        }
        let before = self.items().iter().find(|i| i.id == id).cloned();
        if let LibraryState::Ready(items) = &mut self.state {
            apply(items, id, &op);
        }
        self.pending.push(Pending { id: id.to_string(), op, before });
        self.bump();
        vec![job(user_id, auth_key)]
    }

    /// A write for `id` landed: on success the library is asked again; refused, the entry is put
    /// back as it was and the refusal queued for the toast.
    fn wrote(&mut self, user_id: &str, id: &str, result: Result<(), ApiError>) -> (bool, Vec<LibraryJob>) {
        if !self.user.as_ref().is_some_and(|(u, _)| u == user_id) {
            return (false, Vec::new());
        }
        let Some(at) = self.pending.iter().position(|p| p.id == id) else { return (false, Vec::new()) };
        let done = self.pending.remove(at);
        self.bump();
        match result {
            Ok(()) => {
                // the reload is not skipped by one already on its way: that one may predate the write
                let (user_id, auth_key) = self.user.clone().expect("checked above");
                (true, vec![LibraryJob::Fetch { user_id, auth_key }])
            }
            Err(e) => {
                sn_base::eventlog::log(&format!("library: a write was refused ({e})"));
                if let LibraryState::Ready(items) = &mut self.state {
                    match done.before {
                        Some(before) => match items.iter_mut().find(|i| i.id == id) {
                            Some(i) => *i = before,
                            None => items.push(before),
                        },
                        None => items.retain(|i| i.id != id),
                    }
                }
                self.refused.push(match done.op {
                    Op::Add(_) => Refused::Add,
                    Op::Edit(Edit::Remove) => Refused::Remove,
                    Op::Edit(Edit::Rewind) => Refused::Rewind,
                    Op::Edit(_) | Op::Mark { .. } => Refused::Mark,
                });
                (true, Vec::new())
            }
        }
    }

    /// Record where a playback stands, as of `now` (RFC 3339). Nothing while signed out. The held
    /// entry moves at once by the rules the write applies (`LibraryItem::apply_progress`), so the
    /// episode list wears the watched tick the moment the threshold is crossed.
    // ponytail: not rolled back on a refused write, nor re-applied over a fetch that predates
    // it; the next write or the stop's reload settles it.
    pub fn progress(&mut self, progress: Progress, last: bool, now: &str) -> Vec<LibraryJob> {
        let Some((user_id, auth_key)) = self.user.clone() else { return Vec::new() };
        if let LibraryState::Ready(items) = &mut self.state {
            match items.iter_mut().find(|i| i.id == progress.id) {
                Some(i) => i.apply_progress(&progress, now),
                None => items.push(LibraryItem::progressed_temp(&progress, now)),
            }
            self.bump();
        }
        vec![LibraryJob::Progress { user_id, auth_key, progress, last }]
    }

    /// Where `id` stands for *Add to library* / *Remove from library*.
    pub fn add_state(&self, id: &str) -> AddState {
        match self.pending.iter().find(|p| p.id == id).map(|p| &p.op) {
            Some(Op::Add(_)) => AddState::Adding,
            Some(Op::Edit(Edit::Remove)) => AddState::Removing,
            _ if self.items().iter().any(|i| i.id == id && !i.removed && !i.temp) => AddState::In,
            _ => AddState::Absent,
        }
    }

    pub fn sign_out(&mut self) {
        self.user = None;
        self.pending.clear();
        self.refused.clear();
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
            LibraryLanding::Progressed { user_id, last, result } => {
                if let Err(e) = &result {
                    sn_base::eventlog::log(&format!("library: progress not recorded ({e})"));
                }
                // the last write's reload shows Continue Watching where the viewer stopped
                let Some((u, auth_key)) = self.user.clone().filter(|(u, _)| *u == user_id && last && result.is_ok()) else {
                    return (false, Vec::new());
                };
                return (false, vec![LibraryJob::Fetch { user_id: u, auth_key }]);
            }
            LibraryLanding::Added { user_id, id, result } | LibraryLanding::Edited { user_id, id, result } => {
                return self.wrote(&user_id, &id, result);
            }
        };
        if !self.user.as_ref().is_some_and(|(u, _)| *u == user_id) {
            return (false, Vec::new());
        }
        self.state = match result {
            Ok(mut items) => {
                // a write still on its way is not undone by a fetch that may predate it
                for p in &mut self.pending {
                    p.before = items.iter().find(|i| i.id == p.id).cloned();
                    apply(&mut items, &p.id, &p.op);
                }
                LibraryState::Ready(items)
            }
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
    /// type ([`TYPE_ORDER`] first), in `sort`'s order within a type (stremio-core's comparators).
    pub fn by_type(&self, sort: Sort) -> Vec<(String, Vec<&LibraryItem>)> {
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
            v.sort_by(|a, b| sort.cmp(a, b));
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
    fn recently_watched_channels_are_the_played_ones_most_recent_first() {
        let mut s = LibraryStore::new();
        s.sign_in("u1", &Secret::new("k"));
        let items = vec![
            item(r#"{"_id":"ch_old","type":"tv","name":"Old","removed":true,"temp":true,"state":{"lastWatched":"2026-10-01T00:00:00Z"}}"#),
            item(r#"{"_id":"ch_new","type":"tv","name":"New","poster":"https://img.example/p.png","state":{"lastWatched":"2026-10-24T00:00:00Z"}}"#),
            item(r#"{"_id":"ch_never","type":"tv","name":"Never played"}"#),
            item(r#"{"_id":"tt1","type":"movie","name":"Film","state":{"timeOffset":5,"lastWatched":"2026-10-25T00:00:00Z"}}"#),
        ];
        s.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(items) });
        let ids: Vec<&str> = s.recent_channels().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["ch_new", "ch_old"], "played channels only, a temporary entry too, newest first");
        assert_eq!(s.recent_channels()[0].poster.as_deref(), Some("https://img.example/p.png"));
        assert_eq!(s.recent_channels()[0].kind, "tv");
        assert!(s.continue_watching().iter().all(|i| i.kind != "tv"), "and still none in Continue Watching");
        s.sign_out();
        assert!(s.recent_channels().is_empty(), "signed out: none");
    }

    #[test]
    fn continue_watching_is_in_progress_items_most_recent_first() {
        let s = loaded();
        let ids: Vec<&str> = s.continue_watching().iter().map(|i| i.id.as_str()).collect();
        assert!(ids.contains(&"tmp"), "a temporary entry in progress is continued: {ids:?}");
    }

    #[test]
    fn the_library_groups_by_type_in_the_shared_order() {
        let s = loaded();
        let groups: Vec<(String, Vec<&str>)> =
            s.by_type(Sort::LastWatched).into_iter().map(|(k, v)| (k, v.into_iter().map(|i| i.id.as_str()).collect())).collect();
        assert_eq!(
            groups,
            vec![("movie".into(), vec!["tt1", "tt3"]), ("series".into(), vec!["tt2"]), ("tv".into(), vec!["ch1"])],
            "core's LastWatched: an entry never watched sorts last"
        );
        let names: Vec<&str> = s.by_type(Sort::Name)[0].1.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Old", "Unstarted"]);
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

    fn played(id: &str) -> Progress {
        Progress {
            id: id.into(),
            kind: "movie".into(),
            name: "N".into(),
            poster: None,
            video_id: id.into(),
            time_offset: 1000,
            duration: 2000,
            watched: 1000,
            unload: false,
            videos: None,
        }
    }

    #[test]
    fn progress_is_written_signed_in_and_the_last_write_reloads_the_library() {
        let mut s = loaded();
        let jobs = s.progress(played("tt1"), false, "t");
        assert!(matches!(&jobs[..], [LibraryJob::Progress { progress, last: false, .. }] if progress.time_offset == 1000));
        let ok = |last| LibraryLanding::Progressed { user_id: "u1".into(), last, result: Ok(()) };
        assert!(s.land(ok(false)).1.is_empty(), "a periodic write asks nothing");
        assert!(matches!(&s.land(ok(true)).1[..], [LibraryJob::Fetch { .. }]), "the last write reloads");
        let failed = LibraryLanding::Progressed { user_id: "u1".into(), last: true, result: Err(ApiError::Api { code: 1, message: "x".into() }) };
        assert!(s.land(failed).1.is_empty());
        s.sign_out();
        assert!(s.progress(played("tt1"), true, "t").is_empty(), "nothing is written signed out");
    }

    /// The held entry wears the watched bit the write that crosses 70 % carries, at once.
    #[test]
    fn a_progress_write_moves_the_held_entry_at_once() {
        let mut s = loaded();
        let videos: Vec<sn_stremio::types::Video> =
            (1..=3).map(|e| sn_stremio::types::Video { id: format!("tt2:1:{e}"), season: Some(1), episode: Some(e), ..Default::default() }).collect();
        let p = Progress { id: "tt2".into(), kind: "series".into(), video_id: "tt2:1:2".into(), time_offset: 1500, watched: 1500, videos: Some(videos.clone()), ..played("tt2") };
        let gen = s.gen();
        assert_eq!(s.progress(p, false, "t").len(), 1);
        assert_ne!(s.gen(), gen, "the pages hear of it");
        let held = s.item("tt2").unwrap();
        assert!(held.state.watched_bitfield(&videos).get_video("tt2:1:2"));
        assert_eq!((held.state.times_watched, held.state.time_offset), (1, 1500));
        // a title the library lacks is held as core's temporary entry
        s.progress(played("tt7"), false, "t");
        assert!(s.item("tt7").is_some_and(|i| i.temp && i.state.time_offset == 1000));
    }

    fn refused() -> Result<(), ApiError> {
        Err(ApiError::Api { code: 1, message: "no".into() })
    }

    fn ids(items: &[LibraryItem]) -> Vec<&str> {
        items.iter().filter(|i| !i.removed && !i.temp).map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn a_refused_add_is_rolled_back_reported_once_and_can_be_tried_again() {
        let mut s = loaded();
        let _ = s.add(new("tt9"));
        assert!(ids(s.items()).contains(&"tt9"), "optimistic: the library holds it at once");
        let (changed, jobs) = s.land(LibraryLanding::Added { user_id: "u1".into(), id: "tt9".into(), result: refused() });
        assert!(changed && jobs.is_empty());
        assert_eq!(s.add_state("tt9"), AddState::Absent, "rolled back");
        assert!(!s.items().iter().any(|i| i.id == "tt9"));
        assert_eq!(s.take_failed(), vec![Refused::Add]);
        assert!(s.take_failed().is_empty(), "told once");
        assert_eq!(s.add(new("tt9")).len(), 1, "tried again");
        assert_eq!(s.add_state("tt9"), AddState::Adding);
    }

    #[test]
    fn a_remove_drops_the_entry_at_once_and_a_refusal_puts_it_back() {
        let mut s = loaded();
        assert!(s.remove("tmp").is_empty(), "a temporary entry is not in the library");
        assert!(s.remove("nope").is_empty());
        let jobs = s.remove("tt1");
        assert!(matches!(&jobs[..], [LibraryJob::Edit { id, edit: Edit::Remove, .. }] if id == "tt1"));
        assert_eq!(s.add_state("tt1"), AddState::Removing);
        assert!(!ids(s.items()).contains(&"tt1"), "the Library page drops it at once");
        assert!(!s.continue_watching().iter().any(|i| i.id == "tt1"), "and Continue Watching with it");
        assert!(s.remove("tt1").is_empty() && s.add(new("tt1")).is_empty(), "one write at a time");
        // a fetch that predates the write does not undo it
        let stale = vec![item(r#"{"_id":"tt1","type":"movie","name":"Old","state":{"timeOffset":5}}"#)];
        let _ = s.land(LibraryLanding::Fetched { user_id: "u1".into(), result: Ok(stale) });
        assert_eq!(s.add_state("tt1"), AddState::Removing);
        assert!(ids(s.items()).is_empty());
        let _ = s.land(LibraryLanding::Edited { user_id: "u1".into(), id: "tt1".into(), result: refused() });
        assert_eq!(s.add_state("tt1"), AddState::In, "rolled back");
        assert_eq!(s.continue_watching().iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["tt1"]);
        assert_eq!(s.take_failed(), vec![Refused::Remove]);
    }

    #[test]
    fn a_landed_remove_asks_for_the_library_again() {
        let mut s = loaded();
        let _ = s.remove("tt3");
        let (changed, jobs) = s.land(LibraryLanding::Edited { user_id: "u1".into(), id: "tt3".into(), result: Ok(()) });
        assert!(changed);
        assert!(matches!(&jobs[..], [LibraryJob::Fetch { .. }]));
        assert_eq!(s.add_state("tt3"), AddState::Absent);
        assert!(s.take_failed().is_empty());
    }

    #[test]
    fn a_rewind_drops_the_item_from_continue_watching_and_keeps_it_in_the_library() {
        let mut s = loaded();
        assert!(s.continuing("tt2"));
        assert!(s.rewind("tt3").is_empty(), "not continued: nothing to dismiss");
        let jobs = s.rewind("tt2");
        assert!(matches!(&jobs[..], [LibraryJob::Edit { id, edit: Edit::Rewind, .. }] if id == "tt2"));
        assert!(!s.continuing("tt2"));
        assert_eq!(s.add_state("tt2"), AddState::In, "still in the library");
        let _ = s.land(LibraryLanding::Edited { user_id: "u1".into(), id: "tt2".into(), result: refused() });
        assert!(s.continuing("tt2"), "rolled back");
        assert_eq!(s.take_failed(), vec![Refused::Rewind]);
        s.sign_out();
        assert!(s.rewind("tt2").is_empty() && s.remove("tt2").is_empty(), "nothing is written signed out");
    }

    fn film(id: &str) -> NewEntry {
        NewEntry { id: id.into(), kind: "movie".into(), name: "F".into(), poster: None }
    }

    #[test]
    fn a_mark_is_optimistic_and_a_refusal_puts_the_entry_back() {
        let mut s = loaded();
        let jobs = s.mark(film("tt1"), Edit::MarkWatched(true), "2026-10-17T10:00:00.000Z");
        assert!(matches!(&jobs[..], [LibraryJob::Mark { entry, edit: Edit::MarkWatched(true), .. }] if entry.id == "tt1"));
        let held = s.item("tt1").unwrap();
        assert!(held.watched() && held.state.time_offset == 0, "watched at once, and out of Continue Watching");
        assert!(s.writing("tt1") && s.mark(film("tt1"), Edit::MarkWatched(false), "t").is_empty(), "one write at a time");
        assert_eq!(s.add_state("tt1"), AddState::In, "a mark is not an add or a removal");
        let _ = s.land(LibraryLanding::Edited { user_id: "u1".into(), id: "tt1".into(), result: refused() });
        assert!(!s.item("tt1").unwrap().watched(), "rolled back");
        assert_eq!(s.item("tt1").unwrap().state.time_offset, 5);
        assert_eq!(s.take_failed(), vec![Refused::Mark]);
        assert!(s.mark(film("tt1"), Edit::Remove, "t").is_empty(), "only marks go through `mark`");
    }

    #[test]
    fn marking_a_title_the_library_lacks_holds_a_temporary_entry() {
        let mut s = loaded();
        assert_eq!(s.mark(film("tt9"), Edit::MarkWatched(true), "now").len(), 1);
        let i = s.item("tt9").unwrap();
        assert!(i.temp && i.removed && i.watched(), "core's temporary entry: not in the Library page");
        assert_eq!(s.add_state("tt9"), AddState::Absent);
        let (changed, jobs) = s.land(LibraryLanding::Edited { user_id: "u1".into(), id: "tt9".into(), result: Ok(()) });
        assert!(changed && matches!(&jobs[..], [LibraryJob::Fetch { .. }]));
        s.sign_out();
        assert!(s.mark(film("tt9"), Edit::MarkWatched(true), "now").is_empty(), "nothing is written signed out");
    }
}
