//! **The settings file** — the viewer's preferences, kept across launches: the subtitle size and
//! position (published to the player through `sn_media::prefs`) and the recent searches. (The
//! search addon filter is THE addons store's, persisted beside the account's other files,
//! `account::SEARCH_FILE`; a `search_disabled` an earlier build wrote here is ignored.)
//!
//! One small JSON document, `settings.json` under the persistent state root. It is read ONCE at
//! boot, before the frame loop (the boot's synchronous loads run outside the frame scope), and
//! written whenever the settings or the search store's settings generation moves — never on the
//! frame thread: the snapshot is serialised there and the write (to a temporary file, then a
//! rename, so a crash mid-write leaves the old file) runs on the shared storage worker.
//!
//! A file that is missing, unreadable or malformed is the defaults: a preference is never worth
//! refusing to boot over. Unknown fields are ignored and missing ones default, so the format can
//! grow.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sn_base::eventlog::log;
use sn_media::prefs::{SubtitlePosition, SubtitleSize};
use sn_screens::stores::search::SearchStore;
use sn_screens::stores::settings::{Pref, SettingsStore};

/// The file's shape. Field names are a file-format contract.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
struct File {
    #[serde(default)]
    subtitle_size: SubtitleSize,
    #[serde(default)]
    subtitle_position: SubtitlePosition,
    /// Newest first.
    #[serde(default)]
    recent_searches: Vec<String>,
}

/// Where the settings live: the first private data directory that is writable (the same choice
/// the account cache makes), else the state root.
pub fn path() -> PathBuf {
    sn_base::paths::private_data_candidates()
        .into_iter()
        .find(|dir| crate::account::private_dir(dir))
        .unwrap_or_else(sn_base::paths::persistent_state_root)
        .join("settings.json")
}

/// The stores the file seeds.
pub struct Loaded {
    pub settings: SettingsStore,
    pub search: SearchStore,
}

/// Read `path` (boot, before the frame loop). Anything wrong with it is the defaults, logged.
pub fn load(path: &Path) -> Loaded {
    let file = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<File>(&bytes).unwrap_or_else(|e| {
            log(&format!("settings: unreadable ({e}); using the defaults"));
            File::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
        Err(e) => {
            log(&format!("settings: {e}; using the defaults"));
            File::default()
        }
    };
    of(file)
}

fn of(file: File) -> Loaded {
    Loaded {
        settings: SettingsStore::with(file.subtitle_size.index(), file.subtitle_position.index()),
        search: SearchStore::new(file.recent_searches),
    }
}

/// The file's bytes for the stores as they stand.
pub fn snapshot(settings: &SettingsStore, search: &SearchStore) -> Vec<u8> {
    let file = File {
        subtitle_size: SubtitleSize::from_index(settings.get(Pref::SubtitleSize)),
        subtitle_position: SubtitlePosition::from_index(settings.get(Pref::SubtitlePosition)),
        recent_searches: search.recents().to_vec(),
    };
    serde_json::to_vec_pretty(&file).unwrap_or_default()
}

/// Write `bytes` to `path` on the storage worker: a temporary file beside it, then a rename.
pub fn save(path: PathBuf, bytes: Vec<u8>) {
    let _ticket = sn_base::storage_worker::submit_retained(move || {
        let result = write_atomic(&path, &bytes);
        if let Err(e) = &result {
            log(&format!("settings: not saved ({e})"));
        }
        result.is_ok()
    });
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Hand the player the subtitle preferences in force (boot, and after every change).
pub fn publish(settings: &SettingsStore) {
    sn_media::prefs::set_subtitle_size(SubtitleSize::from_index(settings.get(Pref::SubtitleSize)));
    sn_media::prefs::set_subtitle_position(SubtitlePosition::from_index(settings.get(Pref::SubtitlePosition)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store's ladders ARE the media layer's: the same number of rungs and the same default,
    /// so an index the Settings page writes is the rung the player draws.
    #[test]
    fn the_settings_ladders_are_the_media_layers() {
        assert_eq!(usize::from(Pref::SubtitleSize.rungs()), SubtitleSize::LADDER.len());
        assert_eq!(usize::from(Pref::SubtitlePosition.rungs()), SubtitlePosition::LADDER.len());
        assert_eq!(Pref::SubtitleSize.default_rung(), SubtitleSize::default().index());
        assert_eq!(Pref::SubtitlePosition.default_rung(), SubtitlePosition::default().index());
    }

    /// The install directory is read-only to the running app on a Homebrew Channel install
    /// (`settings: not saved (Permission denied)` on the first beta), so the settings live in the
    /// private data directory the account cache already proved writable.
    #[test]
    fn the_settings_live_in_the_writable_private_data_directory() {
        let p = path();
        assert_eq!(p.file_name().unwrap(), "settings.json");
        let root = sn_base::paths::private_data_candidates().remove(0);
        assert_eq!(p.parent().unwrap(), root, "{p:?}");
    }

    #[test]
    fn the_file_round_trips_the_recents_and_the_subtitles() {
        let dir = sn_base::testscratch::process_dir("settings-file");
        let path = dir.join("settings.json");
        let mut settings = SettingsStore::default();
        settings.set(Pref::SubtitleSize, 3);
        settings.set(Pref::SubtitlePosition, 2);
        let mut search = SearchStore::default();
        search.remember("batman");
        search.remember("news");
        write_atomic(&path, &snapshot(&settings, &search)).unwrap();
        let back = load(&path);
        assert_eq!(back.settings.get(Pref::SubtitleSize), 3);
        assert_eq!(back.settings.get(Pref::SubtitlePosition), 2);
        assert_eq!(back.search.recents(), ["news", "batman"]);
        // an earlier build's filter in this file is read past (the addons store holds it now)
        std::fs::write(&path, br#"{"search_disabled":["x"],"recent_searches":["old"]}"#).unwrap();
        assert_eq!(load(&path).search.recents(), ["old"]);
    }

    #[test]
    fn a_missing_or_broken_file_is_the_defaults() {
        let dir = sn_base::testscratch::process_dir("settings-file-broken");
        let missing = load(&dir.join("absent.json"));
        assert_eq!(missing.settings, SettingsStore::default());
        let broken = dir.join("broken.json");
        std::fs::write(&broken, b"{not json").unwrap();
        assert_eq!(load(&broken).settings, SettingsStore::default());
        let partial = dir.join("partial.json");
        std::fs::write(&partial, br#"{"subtitle_size":"huge","unknown":1}"#).unwrap();
        let p = load(&partial);
        assert_eq!(p.settings.get(Pref::SubtitleSize), 4, "a known field is read, an unknown one ignored");
        assert_eq!(p.settings.get(Pref::SubtitlePosition), Pref::SubtitlePosition.default_rung());
    }
}
