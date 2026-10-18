//! **A series' episode rows** — the season list and the one episode row builder that both the
//! Detail page's episode list (`crate::detail`) and the player's Episodes panel
//! (`crate::player::episodes`) draw, so the two lists cannot drift. Not a screen: the screens
//! compose it.
//!
//! A row is a `sn_ui::table` STILL row: the episode's title (server text), its address and air
//! date under it, its own still in the leading column (decoded at the row's size, only while the
//! row is drawn) and its overview as a third quiet line. An episode whose release date is still
//! ahead reads *Upcoming*, dimmed, instead of looking playable.
//!
//! **Watched marks** ([`Watch`], read off the library entry by [`Watched`]): an episode whose bit
//! is set in the entry's `state.watched` (`sn_stremio::watched`) reads *Watched*; the entry's
//! current video (`state.video_id`) wears the resume bar on its still while it is part-way
//! (stremio-core's `progress`: `timeOffset / duration`). [`opening_episode`] is the episode a
//! series opens on.

use sn_platform::i18n::msg;
use sn_stremio::library::LibraryItem;
use sn_stremio::types::{Meta, Video};
use sn_stremio::watched::{self, WatchedBitField};
use sn_ui::table::Row;

/// A series' seasons in picker order: numbered seasons ascending, specials (season 0) last.
pub fn seasons(meta: &Meta) -> Vec<u32> {
    let mut seasons: Vec<u32> = meta.videos.iter().map(|v| v.season.unwrap_or(0)).collect();
    seasons.sort_by_key(|s| (*s == 0, *s));
    seasons.dedup();
    seasons
}

/// A season's label (its dropdown option and its section heading).
pub fn season_label(season: u32) -> String {
    if season == 0 { msg::app_detail_specials().to_string() } else { msg::app_detail_season(i64::from(season)) }
}

/// `season`'s episodes in episode order, each with its index in `meta.videos`.
pub fn of_season(meta: &Meta, season: u32) -> Vec<(usize, &Video)> {
    let mut videos: Vec<(usize, &Video)> = meta.videos.iter().enumerate().filter(|(_, v)| v.season.unwrap_or(0) == season).collect();
    videos.sort_by_key(|(i, v)| (v.episode.unwrap_or(u32::MAX), *i));
    videos
}

/// The episode's address in `season`: `S1 E5`.
pub fn address(season: u32, v: &Video) -> String {
    sn_ui::fmt::episode_ordinal(i64::from(season), i64::from(v.episode.unwrap_or(0)))
}

/// Has `v` not been released yet, on `today` (`YYYY-MM-DD`; empty marks nothing upcoming)?
pub fn upcoming(v: &Video, today: &str) -> bool {
    let date = v.released.as_deref().map(|d| d.get(..10).unwrap_or(d)).unwrap_or("");
    !today.is_empty() && date > today
}

/// The row of episode `v` of `season`: a chevron when it plays, *Upcoming* dimmed when it is
/// still ahead of `today`.
pub fn row(v: &Video, season: u32, today: &str) -> Row {
    let address = address(season, v);
    let title = if v.title.trim().is_empty() { address.clone() } else { v.title.clone() };
    let date = v.released.as_deref().map(|d| d.get(..10).unwrap_or(d)).unwrap_or("");
    let detail = if date.is_empty() { address } else { format!("{address} \u{b7} {}", sn_ui::fmt::pretty_date(date, 0)) };
    let row = Row::new(title)
        .server_label()
        .detail(detail)
        .server_detail()
        .still(v.thumbnail.as_deref().map(str::trim).unwrap_or(""))
        .blurb(v.overview.as_deref().map(|o| o.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default());
    if upcoming(v, today) { row.value(msg::app_detail_upcoming()).dim(true) } else { row.chevron(true) }
}

/// What the library says of one episode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Watch {
    /// Its watched bit is set.
    pub watched: bool,
    /// It is the entry's current video, this far in (0–1).
    pub progress: Option<f32>,
}

/// A series' library entry and its watched bits over the meta's videos.
pub struct Watched<'a> {
    pub item: Option<&'a LibraryItem>,
    pub bits: WatchedBitField,
}

impl<'a> Watched<'a> {
    /// The entry `items` hold for `item_id`, else the one whose current video `meta` lists
    /// (an item filed under another id), with its bits over `meta`'s videos.
    pub fn of(items: &'a [LibraryItem], item_id: &str, meta: &Meta) -> Self {
        let item = items.iter().find(|i| i.id == item_id).or_else(|| {
            items.iter().find(|i| i.state.video_id.as_deref().is_some_and(|v| meta.videos.iter().any(|m| m.id == v)))
        });
        Watched { item, bits: WatchedBitField::of_state(item.and_then(|i| i.state.watched.as_deref()), &meta.videos) }
    }

    pub fn of_video(&self, v: &Video) -> Watch {
        let s = self.item.map(|i| &i.state);
        let progress = s.filter(|s| s.video_id.as_deref() == Some(v.id.as_str()) && s.time_offset > 0 && s.duration > 0);
        Watch { watched: self.bits.get_video(&v.id), progress: progress.map(|s| (s.time_offset as f32 / s.duration as f32).clamp(0.0, 1.0)) }
    }

    /// Is every episode of `season` watched (stremio-web's `seasonWatched`)?
    pub fn season_watched(&self, meta: &Meta, season: u32) -> bool {
        let eps = of_season(meta, season);
        !eps.is_empty() && eps.iter().all(|(_, v)| self.bits.get_video(&v.id))
    }
}

/// The episode a series opens on: after the last watched one (the entry's current video, else
/// the last its bits mark) the next episode not yet watched ([`watched::next_video`]'s walk:
/// released, never from a season into the specials); that video itself while it is not watched
/// or nothing unwatched follows it. `None` with no last watched video in `meta`.
pub fn opening_episode<'m>(meta: &'m Meta, w: &Watched<'_>, now: &str) -> Option<&'m Video> {
    let in_meta = |id: &str| meta.videos.iter().find(|v| v.id == id);
    let last = w.item.and_then(|i| i.state.video_id.as_deref()).and_then(in_meta).or_else(|| w.bits.last_watched().and_then(in_meta))?;
    let order = watched::core_order(&meta.videos);
    let mut at = last;
    while w.bits.get_video(&at.id) {
        match watched::next_video(&order, &at.id, now) {
            Some(next) => at = next,
            None => return Some(last),
        }
    }
    Some(at)
}

/// [`row`], marked: *Watched* as its read-out once watched, the resume bar on its still.
pub fn row_marked(v: &Video, season: u32, today: &str, watch: Watch) -> Row {
    let mut row = row(v, season, today);
    if watch.watched && !upcoming(v, today) {
        row = row.value(msg::app_watched_watched());
    }
    if let Some(p) = watch.progress {
        row = row.progress(p);
    }
    row
}

/// Now, RFC 3339 (what a video's `released` is compared with).
pub fn now() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    sn_stremio::library::rfc3339_ms(ms)
}

/// Today's date (`YYYY-MM-DD`, UTC) for [`row`]; empty while the set's clock is plainly unset
/// (before 2024), so a wrong clock marks nothing upcoming.
pub fn today() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    if ms < 1_704_067_200_000 {
        return String::new();
    }
    sn_stremio::library::rfc3339_ms(ms).get(..10).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(s: u32, e: u32) -> Video {
        Video { id: format!("x:{s}:{e}"), season: Some(s), episode: Some(e), ..Default::default() }
    }

    fn meta() -> Meta {
        Meta { videos: vec![ep(1, 1), ep(1, 2), ep(2, 1), ep(2, 2), ep(0, 1)], ..Default::default() }
    }

    fn entry(video: Option<&str>, watched: &[&str]) -> LibraryItem {
        let m = meta();
        let mut bits = WatchedBitField::of_state(None, &m.videos);
        for id in watched {
            bits.set_video(id, true);
        }
        let mut i: LibraryItem = serde_json::from_str(r#"{"_id":"x","type":"series"}"#).unwrap();
        i.state.video_id = video.map(str::to_string);
        i.state.watched = Some(bits.to_string());
        i
    }

    const NOW: &str = "2026-10-17T00:00:00.000Z";

    #[test]
    fn a_series_opens_on_the_next_unwatched_episode_after_the_last_watched() {
        let m = meta();
        let open = |i: LibraryItem| {
            let items = [i];
            opening_episode(&m, &Watched::of(&items, "x", &m), NOW).map(|v| v.id.clone())
        };
        assert_eq!(open(entry(Some("x:1:2"), &["x:1:1", "x:1:2"])).as_deref(), Some("x:2:1"), "the next season's first");
        assert_eq!(open(entry(Some("x:1:1"), &["x:1:1", "x:1:2"])).as_deref(), Some("x:2:1"), "past the watched ones");
        assert_eq!(open(entry(Some("x:1:2"), &["x:1:1"])).as_deref(), Some("x:1:2"), "the current video is not watched yet");
        assert_eq!(open(entry(Some("x:2:2"), &["x:2:2"])).as_deref(), Some("x:2:2"), "nothing follows: the last watched");
        assert_eq!(open(entry(None, &["x:1:1"])).as_deref(), Some("x:1:2"), "no current video: after the bits' last");
        assert_eq!(open(entry(None, &[])), None);
    }

    #[test]
    fn an_episode_reads_watched_and_the_current_one_its_progress() {
        let _en = sn_platform::i18n::language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        let m = meta();
        let mut i = entry(Some("x:1:2"), &["x:1:1"]);
        i.state.time_offset = 600_000;
        i.state.duration = 2_400_000;
        let items = [i];
        let w = Watched::of(&items, "x", &m);
        assert_eq!(w.of_video(&m.videos[0]), Watch { watched: true, progress: None });
        assert_eq!(w.of_video(&m.videos[1]), Watch { watched: false, progress: Some(0.25) });
        assert!(!w.season_watched(&m, 1));
        let row = row_marked(&m.videos[0], 1, "", w.of_video(&m.videos[0]));
        assert_eq!(row.value.as_deref(), Some("Watched"));
        assert_eq!(row_marked(&m.videos[1], 1, "", w.of_video(&m.videos[1])).progress, Some(0.25));
    }

    /// The marked episode row fits a detail page's list, in every shipped language.
    #[test]
    fn a_watched_episode_row_fits_in_every_shipped_language() {
        use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let mut v = ep(1, 1);
            v.title = "Pilot".into();
            v.released = Some("2008-01-20T00:00:00.000Z".into());
            let mut t = sn_ui::table::TableView::new();
            t.sections = vec![sn_ui::table::Section::new(season_label(1)).row(row_marked(&v, 1, "", Watch { watched: true, progress: Some(0.5) }))];
            out.extend(t.app_fit_failures(crate::detail::detail_layout().sectioned_table().w, language.tag()));
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }
}
