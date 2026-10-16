//! **Detail** — one item: its description, then either its episodes (a series, a channel) or
//! the streams every addon offers for it, one section per addon. The same screen in its second
//! mode is the **Streams** page of one episode.
//!
//! The page is a `sn_ui::table_screen::TableScreen`: the route header (crumb, title, copy) beside
//! a `TableView`. The table's sections are built by [`build`], a pure function of what the detail
//! store holds, so the per-addon states are graded by the host suite:
//!
//! - an addon still being asked reads "Looking for streams…" under its name;
//! - an addon that failed reads its own failure, with the cause;
//! - an addon that answered nothing reads "No streams from this addon";
//! - an addon's streams are rows; one this app can play (HTTP, with the addon's request headers)
//!   starts the player on OK, one it cannot (a torrent, a link elsewhere) is shown dimmed.
//!
//! Rows are focused by IDENTITY ([`RowKeys`]): a stream row's key is its addon and position, so a
//! slower addon landing above the focused row moves no focus.

use std::borrow::Cow;

use sn_machine::machine::{Canon, Cx, Effects, EntryId, Fx, GroupId, Handled, Leave, LogicalState, Machine, NavOp};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_stremio::streams::{GroupState, StreamGroup};
use sn_stremio::types::{Meta, Playable, Video};
use sn_ui::form::{RowKey, RowKeys};
use sn_ui::route_screen::RouteLayout;
use sn_ui::screen::{DrawFrame, FocusSource, HitSource, Part, RenderStrategy, Screen, ScreenEvent};
use sn_ui::table::{Row, Section, TableView};
use sn_ui::table_screen::{Header, TableScreen};
use sn_ui::{theme, Rect};

use crate::registry::{word, AppArg, AppFx, Bundle, DetailCmd, FirstSeat, ItemRef, PlayRequest, VideoRef};
use crate::stores::detail::{MetaKey, MetaState, StreamKey};
use crate::stores::StoreId;

const STREAM_KEY: u32 = 0x0010_0000;
const EPISODE_KEY: u32 = 0x0800_0000;
const NOTE_KEY: u32 = 0x0F00_0000;
/// Streams shown per addon (an addon answering more is cut, never overflowing a key).
pub const MAX_STREAMS_PER_ADDON: usize = 0x0FFF;
/// Addons shown on one page.
pub const MAX_STREAM_ADDONS: usize = 0xFF;

/// What a row does when it is activated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open this episode's Streams page.
    Episode(VideoRef),
    Play(PlayRequest),
}

/// The table's content and, per global row, its key and what it does.
#[derive(Default)]
pub struct Built {
    pub sections: Vec<Section>,
    pub keys: Vec<RowKey>,
    pub actions: Vec<Option<Action>>,
}

impl Built {
    fn note(&mut self, section: &mut Section, text: impl Into<String>) {
        section.rows.push(Row::note(text));
        self.keys.push(RowKey(NOTE_KEY + self.keys.len() as u32));
        self.actions.push(None);
    }

    fn row(&mut self, section: &mut Section, row: Row, key: u32, action: Option<Action>) {
        section.rows.push(row);
        self.keys.push(RowKey(key));
        self.actions.push(action);
    }

    pub fn index_of(&self, key: RowKey) -> Option<usize> {
        self.keys.iter().position(|k| *k == key)
    }
}

impl RowKeys for Built {
    fn key_at(&self, index: usize) -> Option<RowKey> {
        self.keys.get(index).copied()
    }
    fn index_of_key(&self, key: RowKey) -> Option<usize> {
        Built::index_of(self, key)
    }
    /// Keys are identities, so a rebuild never lands the page on a row other than the one the
    /// engine's key names: nothing to re-seat.
    fn reseat(&self) -> Option<RowKey> {
        None
    }
}

/// The page's modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Item(ItemRef),
    Video(VideoRef),
    /// A Continue Watching item with no recorded video, until its meta names the first episode
    /// ([`resume_video`]); the page is then that episode's Streams page.
    Resume(ItemRef),
}

impl Mode {
    /// Does the item list episodes rather than having streams of its own? A series and a channel
    /// do; anything else (a movie, a live channel) plays its own id.
    fn lists_episodes(kind: &str) -> bool {
        matches!(kind, "series" | "channel")
    }

    fn meta_key(&self) -> Option<MetaKey> {
        match self {
            Mode::Item(i) | Mode::Resume(i) => Some(MetaKey { kind: i.kind.clone(), id: i.id.clone() }),
            Mode::Video(_) => None,
        }
    }

    /// The streams the page opens at once (without waiting for the meta).
    fn stream_key(&self) -> Option<StreamKey> {
        match self {
            Mode::Item(i) if Self::lists_episodes(&i.kind) => None,
            Mode::Item(i) => Some(StreamKey { kind: i.kind.clone(), video_id: i.id.clone() }),
            Mode::Video(v) => Some(StreamKey { kind: v.kind.clone(), video_id: v.video_id.clone() }),
            Mode::Resume(_) => None,
        }
    }
}

/// What a resume with no recorded video opens once `meta` is in: the first episode
/// (`sn_stremio::library::first_episode`), or the item's own id when the meta lists no video.
/// Never the bare series id while the meta has episodes.
pub fn resume_video(item: &ItemRef, meta: &Meta) -> VideoRef {
    let show = if meta.preview.name.is_empty() { item.name.clone() } else { meta.preview.name.clone() };
    match sn_stremio::library::first_episode(&meta.videos) {
        Some(v) => {
            let address = msg::core_episode_ordinal(i64::from(v.season.unwrap_or(0)), i64::from(v.episode.unwrap_or(0)));
            let title = if v.title.trim().is_empty() { address } else { format!("{address} {}", v.title) };
            VideoRef { kind: item.kind.clone(), video_id: v.id.clone(), show, title, resume_ms: 0 }
        }
        None => VideoRef { kind: item.kind.clone(), video_id: item.id.clone(), show, title: String::new(), resume_ms: 0 },
    }
}

/// The table for `mode`, from what the store holds.
pub fn build(mode: &Mode, meta: Option<&MetaState>, streams: Option<&[StreamGroup]>) -> Built {
    let mut out = Built::default();
    if let Mode::Resume(_) = mode {
        // until the meta names the episode: its state, as the episodes section reads it
        let note = match meta {
            Some(MetaState::Failed(e)) => msg::app_detail_episodes_failed(&e.to_string()),
            Some(MetaState::NoSource) => msg::app_detail_no_meta().to_string(),
            _ => msg::app_detail_episodes_loading().to_string(),
        };
        let mut s = Section::new(msg::app_detail_episodes());
        out.note(&mut s, note);
        out.sections.push(s);
        return out;
    }
    if let Mode::Item(item) = mode {
        if Mode::lists_episodes(&item.kind) {
            match meta {
                Some(MetaState::Ready(m)) if !m.videos.is_empty() => episodes(&mut out, item, m),
                // a series whose meta lists no video plays its own id: its streams follow
                Some(MetaState::Ready(_)) if streams.is_some() => {}
                Some(MetaState::Ready(_)) => {
                    let mut s = Section::new(msg::app_detail_episodes());
                    out.note(&mut s, msg::app_detail_no_episodes());
                    out.sections.push(s);
                }
                Some(MetaState::Failed(e)) => {
                    let mut s = Section::new(msg::app_detail_episodes());
                    out.note(&mut s, msg::app_detail_episodes_failed(&e.to_string()));
                    out.sections.push(s);
                }
                Some(MetaState::NoSource) => {
                    let mut s = Section::new(msg::app_detail_episodes());
                    out.note(&mut s, msg::app_detail_no_meta());
                    out.sections.push(s);
                }
                Some(MetaState::Loading) | None => {
                    let mut s = Section::new(msg::app_detail_episodes());
                    out.note(&mut s, msg::app_detail_episodes_loading());
                    out.sections.push(s);
                }
            }
        }
    }
    if let Some(groups) = streams {
        stream_sections(&mut out, groups, mode);
    }
    out
}

/// The play title for `mode`: what the player page shows.
fn play_title(mode: &Mode) -> String {
    match mode {
        Mode::Item(i) | Mode::Resume(i) => i.name.clone(),
        Mode::Video(v) if v.title.is_empty() => v.show.clone(),
        Mode::Video(v) => format!("{} \u{2014} {}", v.show, v.title),
    }
}

fn stream_sections(out: &mut Built, groups: &[StreamGroup], mode: &Mode) {
    if groups.is_empty() {
        let mut s = Section::new(msg::app_detail_streams());
        out.note(&mut s, msg::app_detail_no_stream_addons());
        out.sections.push(s);
        return;
    }
    for (gi, group) in groups.iter().take(MAX_STREAM_ADDONS).enumerate() {
        let mut s = Section::new(group.addon_name.clone()).server_header();
        match &group.state {
            GroupState::Loading => out.note(&mut s, msg::app_detail_streams_loading()),
            GroupState::Failed(e) => out.note(&mut s, msg::app_detail_streams_failed(&e.to_string())),
            GroupState::Ready(streams) if streams.is_empty() => out.note(&mut s, msg::app_detail_no_streams()),
            GroupState::Ready(streams) => {
                for (si, stream) in streams.iter().take(MAX_STREAMS_PER_ADDON).enumerate() {
                    let (name, detail) = stream.label();
                    let label = if name.trim().is_empty() { group.addon_name.clone() } else { one_line(&name) };
                    let mut row = Row::new(label).server_label().detail(one_line(&detail)).server_detail();
                    let action = match stream.playable() {
                        Some(Playable::Http { url, headers }) => {
                            let start_ms = if let Mode::Video(v) = mode { v.resume_ms } else { 0 };
                            Some(Action::Play(PlayRequest { url, headers, title: play_title(mode), start_ms }))
                        }
                        _ => {
                            row = row.value(msg::app_detail_not_playable()).dim(true);
                            None
                        }
                    };
                    out.row(&mut s, row, STREAM_KEY + ((gi as u32) << 12) + si as u32, action);
                }
            }
        }
        out.sections.push(s);
    }
}

/// An addon's multi-line label (`"1080p\n👤 42"`) as one row line.
fn one_line(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" \u{b7} ")
}

/// A series' episodes, one section per season (specials last), in episode order.
fn episodes(out: &mut Built, item: &ItemRef, meta: &Meta) {
    let mut seasons: Vec<u32> = meta.videos.iter().map(|v| v.season.unwrap_or(0)).collect();
    seasons.sort_by_key(|s| (*s == 0, *s));
    seasons.dedup();
    let show = if meta.preview.name.is_empty() { item.name.clone() } else { meta.preview.name.clone() };
    for season in seasons {
        let header = if season == 0 { msg::app_detail_specials().to_string() } else { msg::app_detail_season(i64::from(season)) };
        let mut s = Section::new(header);
        let mut videos: Vec<(usize, &Video)> =
            meta.videos.iter().enumerate().filter(|(_, v)| v.season.unwrap_or(0) == season).collect();
        videos.sort_by_key(|(i, v)| (v.episode.unwrap_or(u32::MAX), *i));
        for (i, v) in videos {
            let address = msg::core_episode_ordinal(i64::from(season), i64::from(v.episode.unwrap_or(0)));
            let title = if v.title.trim().is_empty() { address.clone() } else { v.title.clone() };
            let date = v.released.as_deref().map(|d| d.get(..10).unwrap_or(d)).unwrap_or("");
            let detail = if date.is_empty() { address.clone() } else { format!("{address} \u{b7} {date}") };
            let row = Row::new(title.clone()).server_label().detail(detail).server_detail().chevron(true);
            let action = Action::Episode(VideoRef {
                kind: item.kind.clone(),
                video_id: v.id.clone(),
                show: show.clone(),
                title: if v.title.trim().is_empty() { address } else { format!("{address} {title}") },
                resume_ms: 0,
            });
            out.row(&mut s, row, EPISODE_KEY + (i as u32).min(0x00FF_FFFF), Some(action));
        }
        out.sections.push(s);
    }
}

pub struct DetailScreen {
    entry: EntryId,
    mode: Mode,
    table: TableView,
    built: Built,
    /// What the store held at the last build.
    built_gen: Option<u32>,
    /// The streams this page holds open: the item's own, or a series' that listed no video.
    streams: Option<StreamKey>,
    /// The meta this page holds open.
    meta: Option<MetaKey>,
    /// The focused row's identity.
    sel: Option<RowKey>,
    title: String,
    copy: String,
    crumb: String,
    closed: bool,
    /// The page mounts on read-outs (the meta and the streams still out): the first row that
    /// lands takes focus.
    seat: FirstSeat,
}

impl DetailScreen {
    pub fn item(entry: EntryId, item: ItemRef) -> Self {
        Self::new(entry, Mode::Item(item))
    }

    pub fn video(entry: EntryId, video: VideoRef) -> Self {
        Self::new(entry, Mode::Video(video))
    }

    /// A resume with no recorded video: the first episode's streams once the meta names it.
    pub fn resume(entry: EntryId, item: ItemRef) -> Self {
        Self::new(entry, Mode::Resume(item))
    }

    fn new(entry: EntryId, mode: Mode) -> Self {
        let (title, crumb) = match &mode {
            Mode::Item(i) | Mode::Resume(i) => (i.name.clone(), msg::app_nav_home().to_string()),
            Mode::Video(v) => (if v.title.is_empty() { v.show.clone() } else { v.title.clone() }, v.show.clone()),
        };
        let mut table = TableView::new();
        table.list_focused = true;
        DetailScreen {
            entry,
            mode,
            table,
            built: Built::default(),
            built_gen: None,
            streams: None,
            meta: None,
            sel: None,
            title,
            copy: String::new(),
            crumb,
            closed: false,
            seat: FirstSeat::new(),
        }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// The action of the row `key` names.
    pub fn action_of(&self, key: u32) -> Option<&Action> {
        let i = self.built.index_of(RowKey(key))?;
        self.built.actions.get(i)?.as_ref()
    }

    /// The built table, as the test suite reads it.
    pub fn sections(&self) -> &[Section] {
        &self.table.sections
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::screen(), Some(self.crumb.as_str()), &self.title, &self.copy),
            &self.table,
            GroupId(0),
            self.entry,
        )
        .keyed(&self.built)
    }

    fn open(&mut self, fx: &mut Effects<'_, Bundle>) {
        if let Some(key) = self.mode.meta_key() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenMeta(key.clone()))));
            self.meta = Some(key);
        }
        if let Some(key) = self.mode.stream_key() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenStreams(key.clone()))));
            self.streams = Some(key);
        }
    }

    fn close(&mut self, fx: &mut Effects<'_, Bundle>) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        if let Some(key) = self.meta.take() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::CloseMeta(key))));
        }
        if let Some(key) = self.streams.take() {
            fx.push(Fx::App(AppFx::Detail(DetailCmd::CloseStreams(key))));
        }
    }

    /// Rebuild the table from the store when it moved; keep the focused row by identity.
    fn sync(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let store = cx.views.detail;
        // a resume's meta landed: this is now the first episode's Streams page
        if let (Mode::Resume(item), Some(MetaState::Ready(m))) = (&self.mode, self.meta.as_ref().and_then(|k| store.meta(k))) {
            if !self.closed {
                let video = resume_video(item, m);
                let key = StreamKey { kind: video.kind.clone(), video_id: video.video_id.clone() };
                fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenStreams(key.clone()))));
                self.streams = Some(key);
                if !video.title.is_empty() {
                    self.title = video.title.clone();
                }
                self.mode = Mode::Video(video);
                self.built_gen = None;
            }
        }
        // a series whose meta lists no video plays its own id
        let no_videos = matches!(self.meta.as_ref().and_then(|k| store.meta(k)), Some(MetaState::Ready(m)) if m.videos.is_empty());
        if no_videos && self.streams.is_none() && !self.closed {
            if let Mode::Item(item) = &self.mode {
                let key = StreamKey { kind: item.kind.clone(), video_id: item.id.clone() };
                fx.push(Fx::App(AppFx::Detail(DetailCmd::OpenStreams(key.clone()))));
                self.streams = Some(key);
            }
        }
        if self.built_gen == Some(store.gen()) {
            return;
        }
        self.built_gen = Some(store.gen());
        let meta = self.meta.as_ref().and_then(|k| store.meta(k));
        if let Some(MetaState::Ready(m)) = meta {
            if !m.preview.name.is_empty() && matches!(self.mode, Mode::Item(_)) {
                self.title = m.preview.name.clone();
            }
            self.copy = describe(m);
        }
        let streams = self.streams.as_ref().and_then(|k| store.streams(k));
        let built = build(&self.mode, meta, streams);
        let sel = self
            .sel
            .and_then(|k| built.index_of(k))
            .map(|i| i as i32);
        let fresh = self.table.sections.is_empty();
        self.built = built;
        let sections = std::mem::take(&mut self.built.sections);
        self.table.set_sections_or_open(sections, sel, !fresh);
        fx.invalidate(Provenance::Landing(fx.from()));
    }
}

/// The header's copy line: the meta's description, or its release year and genres.
fn describe(m: &Meta) -> String {
    if let Some(d) = m.preview.description.as_deref().filter(|d| !d.trim().is_empty()) {
        return d.trim().to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    if let Some(r) = m.preview.release_info.as_deref() {
        parts.push(r);
    }
    parts.extend(m.preview.genres.iter().map(String::as_str));
    parts.join(" \u{b7} ")
}

impl LogicalState for DetailScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
        c.option(self.sel, |c, k| {
            c.u32(k.0);
        });
        c.bool(self.closed);
        self.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("detail rows={} sel={:?}", self.table.n_rows(), self.sel));
    }
}

impl Machine<Bundle> for DetailScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.open(fx);
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Detail.ord() => {
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.sync(cx, fx);
                let selectable = self.built.actions.iter().any(Option::is_some);
                let target = selectable.then_some(sn_ui::screen::FocusTarget::FirstInGroup(GroupId(0)));
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == self.entry), target, fx);
                self.table.update(t.dt(), RouteLayout::screen().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.built.index_of(RowKey(to.elem)) {
                    self.table.sel = i as i32;
                    self.sel = Some(RowKey(to.elem));
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                match self.action_of(*elem).cloned() {
                    Some(Action::Episode(video)) => fx.push(Fx::Nav(NavOp::Push(AppArg::Streams(video)))),
                    Some(Action::Play(req)) => fx.push(Fx::App(AppFx::Play(req))),
                    None => {}
                }
                Handled::Yes
            }
            ScreenEvent::WillLeave(Leave::ForGood) | ScreenEvent::Unmount => {
                self.close(fx);
                Handled::Yes
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

sn_ui::focusable_via_view!(DetailScreen, Bundle, view);

impl Screen<Bundle> for DetailScreen {
    fn name(&self) -> &'static str {
        match self.mode {
            Mode::Item(_) => word::DETAIL,
            Mode::Video(_) => word::STREAMS,
            Mode::Resume(_) => word::RESUME,
        }
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(&self.crumb))
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        let mut v = self.view();
        Part::<Bundle>::draw(&mut v, f, Rect::FULL);
    }
    fn render(&self) -> RenderStrategy {
        RenderStrategy::Page
    }
    fn focus_source(&self) -> FocusSource {
        FocusSource::Engine
    }
    fn hit_source(&self) -> HitSource {
        HitSource::Engine
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_stremio::types::{MetaPreview, ProxyHeaders, Stream, StreamHints};
    use sn_stremio::FetchError;

    fn movie() -> Mode {
        Mode::Item(ItemRef { kind: "movie".into(), id: "tt1".into(), name: "Film".into() })
    }

    fn group(name: &str, state: GroupState) -> StreamGroup {
        StreamGroup { addon_id: name.to_lowercase(), addon_name: name.into(), url: String::new(), state }
    }

    fn http(name: &str, title: &str, url: &str) -> Stream {
        Stream { url: Some(url.into()), name: Some(name.into()), title: Some(title.into()), ..Default::default() }
    }

    fn texts(b: &Built) -> Vec<(String, Vec<String>)> {
        b.sections.iter().map(|s| (s.header.clone(), s.rows.iter().map(|r| r.label.clone()).collect())).collect()
    }

    #[test]
    fn every_addon_shows_its_own_state() {
        let groups = vec![
            group("Fast", GroupState::Ready(vec![http("4K", "Film 2160p\n👤 12", "https://cdn.example/a.mkv")])),
            group("Slow", GroupState::Loading),
            group("Broken", GroupState::Failed(FetchError::Status(503))),
            group("Empty", GroupState::Ready(Vec::new())),
        ];
        let b = build(&movie(), None, Some(&groups));
        assert_eq!(
            texts(&b),
            vec![
                ("Fast".into(), vec!["4K".into()]),
                ("Slow".into(), vec![msg::app_detail_streams_loading().into()]),
                ("Broken".into(), vec![msg::app_detail_streams_failed("HTTP 503")]),
                ("Empty".into(), vec![msg::app_detail_no_streams().into()]),
            ]
        );
        assert_eq!(b.sections[0].rows[0].detail, "Film 2160p \u{b7} 👤 12", "the addon's lines are one row line");
        assert!(b.sections[1].rows[0].is_note(), "a state is a note, never a focusable row");
        assert_eq!(b.actions.iter().filter(|a| a.is_some()).count(), 1);
    }

    #[test]
    fn an_http_stream_plays_with_its_request_headers_and_a_torrent_does_not() {
        let mut live = http("Live", "", "https://live.example/ch1.m3u8");
        live.behavior_hints = StreamHints {
            proxy_headers: Some(ProxyHeaders {
                request: [("Referer".to_string(), "https://live.example/".to_string())].into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let torrent = Stream { info_hash: Some("ABC".into()), name: Some("Torrent".into()), ..Default::default() };
        let groups = vec![group("TV", GroupState::Ready(vec![live, torrent]))];
        let b = build(&movie(), None, Some(&groups));
        match &b.actions[0] {
            Some(Action::Play(req)) => {
                assert_eq!(req.url, "https://live.example/ch1.m3u8");
                assert_eq!(req.headers, vec![("Referer".to_string(), "https://live.example/".to_string())]);
                assert_eq!(req.title, "Film");
            }
            other => panic!("expected a play action, got {other:?}"),
        }
        assert_eq!(b.actions[1], None, "a torrent needs a streaming server");
        assert!(b.sections[0].rows[1].dim);
        assert_eq!(b.sections[0].rows[1].value.as_deref(), Some(msg::app_detail_not_playable()));
    }

    #[test]
    fn a_landing_above_the_focused_row_keeps_its_key() {
        let fast = group("Fast", GroupState::Ready(vec![http("A", "", "https://x/a"), http("B", "", "https://x/b")]));
        let before = build(&movie(), None, Some(&[group("Slow", GroupState::Loading), fast.clone()]));
        let focused = before.keys[2]; // "B" of the second addon
        let after = build(
            &movie(),
            None,
            Some(&[group("Slow", GroupState::Ready(vec![http("C", "", "https://x/c"), http("D", "", "https://x/d")])), fast]),
        );
        let at = after.index_of(focused).expect("the focused stream keeps its key");
        assert_eq!(after.sections[1].rows[1].label, "B");
        assert_eq!(at, 3, "and is found at its new index");
    }

    #[test]
    fn no_streaming_addon_says_so_rather_than_showing_nothing() {
        let b = build(&movie(), None, Some(&[]));
        assert_eq!(texts(&b), vec![(msg::app_detail_streams().into(), vec![msg::app_detail_no_stream_addons().into()])]);
    }

    #[test]
    fn a_series_lists_its_episodes_by_season_with_specials_last() {
        let item = ItemRef { kind: "series".into(), id: "tt0903747".into(), name: "Show".into() };
        let v = |id: &str, s: u32, e: u32, t: &str| Video {
            id: id.into(),
            title: t.into(),
            season: Some(s),
            episode: Some(e),
            released: Some("2008-01-20T00:00:00.000Z".into()),
            ..Default::default()
        };
        let meta = Meta {
            preview: MetaPreview { id: "tt0903747".into(), kind: "series".into(), name: "Breaking".into(), ..Default::default() },
            videos: vec![v("x:0:1", 0, 1, "Special"), v("x:1:2", 1, 2, "Two"), v("x:1:1", 1, 1, "Pilot"), v("x:2:1", 2, 1, "")],
            ..Default::default()
        };
        let loading = build(&Mode::Item(item.clone()), Some(&MetaState::Loading), None);
        assert_eq!(texts(&loading), vec![(msg::app_detail_episodes().into(), vec![msg::app_detail_episodes_loading().into()])]);
        let b = build(&Mode::Item(item.clone()), Some(&MetaState::Ready(Box::new(meta))), None);
        let t = texts(&b);
        assert_eq!(t[0], (msg::app_detail_season(1), vec!["Pilot".to_string(), "Two".to_string()]));
        assert_eq!(t[1].1, vec![msg::core_episode_ordinal(2, 1)], "an untitled episode reads as its address");
        assert_eq!(t[2], (msg::app_detail_specials().to_string(), vec!["Special".to_string()]));
        assert_eq!(b.sections[0].rows[0].detail, format!("{} \u{b7} 2008-01-20", msg::core_episode_ordinal(1, 1)));
        match &b.actions[0] {
            Some(Action::Episode(video)) => {
                assert_eq!(video.video_id, "x:1:1");
                assert_eq!(video.show, "Breaking");
                assert_eq!(video.kind, "series");
            }
            other => panic!("expected an episode, got {other:?}"),
        }
        let failed = build(&Mode::Item(item), Some(&MetaState::Failed(FetchError::Status(404))), None);
        assert_eq!(failed.sections[0].rows[0].label, msg::app_detail_episodes_failed("HTTP 404"));
    }
}
