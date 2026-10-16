//! **The application bundle** (restructure spec §3.1): the one [`Host`] the dispatcher, the
//! containers and every screen here are compiled against — the screen argument ([`AppArg`]), the
//! effects a screen may ask of the application ([`AppFx`]), the read side ([`Views`]) — and the
//! one `mount` match ([`AppMounter`]).
//!
//! A new screen touches its own module, a variant of [`AppArg`] and an arm of
//! [`AppMounter::mount`] here, and nothing else in this crate. A new TOP-BAR destination also
//! takes one line in [`crate::tabs::TABS`], which is what the bar, the colour/channel hotkeys and
//! BACK all read.

use std::ffi::CStr;

use sn_machine::machine::{
    Canon, Chrome, Cx, Effects, EntryId, FocusKey, Host, InputOwner, InstanceId, LogicalState, MachineId, ScreenArg,
    ScreenId,
};
use sn_ui::screen::{Mounter, ReturnState, Screen};

use crate::stores::catalog::{CatalogStore, RowId};
use crate::stores::detail::{DetailStore, MetaKey, StreamKey};
use crate::stores::grid::{GridKey, GridStore};
use crate::stores::search::SearchStore;
use crate::stores::settings::{Pref, SettingsStore};

/// The bundle.
pub struct Bundle;

impl Host for Bundle {
    type Arg = AppArg;
    type Fx = AppFx;
    type Msg = AppMsg;
    type Elem = u32;
    type Views<'a> = Views<'a>;
    type Init = NoInit;
    /// No page remembers anything beyond what the engine already keeps (its focus and cursors).
    type Memory = ();
}

/// The heartbeat words — what each screen's `name()` answers.
pub mod word {
    pub const HOME: &str = "home";
    pub const SEARCH: &str = "search";
    pub const LIVE_TV: &str = "livetv";
    pub const LIBRARY: &str = "library";
    pub const ADDONS: &str = "addons";
    pub const SETTINGS: &str = "settings";
    pub const SETTINGS_PAGE: &str = "settings-page";
    pub const CATALOG: &str = "catalog";
    pub const MENU: &str = "menu";
    pub const DETAIL: &str = "detail";
    pub const STREAMS: &str = "streams";
    pub const PLAYER: &str = "player";
}

/// An item a catalog showed, as a Detail page opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRef {
    /// The Stremio type (`movie`, `series`, `tv`, `channel`, …).
    pub kind: String,
    pub id: String,
    /// The catalog's own name for it, shown before the meta lands. Server text.
    pub name: String,
}

/// One video's streams, as a Streams page opens them: a series episode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoRef {
    pub kind: String,
    pub video_id: String,
    /// The show's name: the crumb BACK returns to. Server text.
    pub show: String,
    /// The page's title (the episode's own title, or its address). Server text.
    pub title: String,
}

/// One catalog of one addon, opened as a full paginated grid: a search row's MORE. The addon is
/// named by its manifest id and resolved against the installed addons when the page opens (an
/// addon's URL may carry its configuration, so it never rides on an argument).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRef {
    pub key: GridKey,
    /// The page's title: the addon and catalog names. Server text.
    pub title: String,
}

/// The Settings family's pages under the root (each a push on the shared stack).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    /// Pick one rung of a preference ladder.
    Picker(Pref),
    /// The legal notices: the third-party components and the full licence texts.
    Legal,
    /// A document to read.
    Doc(Doc),
}

/// A Settings document (`crate::settings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Doc {
    /// The remote's key map (`docs/remote-keys.md` in the viewer's words).
    Remote,
    /// About StremioNative: not affiliated with Stremio, the GPL and the source address, the
    /// PlxNative credit, the version.
    About,
    /// What leaves the television, and where it goes (`PRIVACY.md`).
    Privacy,
    /// The third-party components and their licences (`docs/legal.md`).
    Components,
    /// The GNU General Public License, version 3 (the package's `LICENSE`).
    Gpl,
    /// The SIL Open Font License of the bundled fonts (the package's `OFL.txt`).
    Ofl,
    /// The copyright notice (the package's `NOTICE`).
    Notice,
}

impl SettingsPage {
    fn ordinal(self) -> u32 {
        match self {
            SettingsPage::Picker(p) => p as u32,
            SettingsPage::Legal => 0x10,
            SettingsPage::Doc(d) => 0x20 + d as u32,
        }
    }
}

/// A popover menu's content (`crate::menu`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// A card's item menu: Open, and Add to library (disabled until signed in).
    Item(ItemRef),
    /// Pick one of `options` (the current one checked); the pick goes back to the presenting page
    /// as [`AppMsg::Chose`] with `target`. Options are server text (catalog names, genres).
    Choice { title: String, options: Vec<String>, current: usize, target: ChoiceTarget },
}

/// Which control a [`MenuKind::Choice`] answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceTarget {
    /// The Live TV page's catalog chip.
    LiveCatalog,
    /// The Live TV page's genre chip.
    LiveGenre,
}

/// A popover menu, anchored beside the control it was opened from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuArg {
    /// The page the menu hangs off.
    pub host: EntryId,
    /// That page's machine (its `fx.from()` when it presented the menu): a choice is delivered
    /// back to it.
    pub reply: MachineId,
    /// The element the menu was opened from.
    pub focus: Option<FocusKey<u32>>,
    /// The opener's rect, as `f32` bits (an argument is `Eq`).
    pub anchor: [u32; 4],
    pub kind: MenuKind,
}

impl MenuArg {
    pub fn anchor_rect(&self) -> sn_ui::Rect {
        let [x, y, w, h] = self.anchor.map(f32::from_bits);
        sn_ui::Rect::new(x, y, w, h)
    }

    pub fn anchor_of(r: sn_ui::Rect) -> [u32; 4] {
        [r.x, r.y, r.w, r.h].map(f32::to_bits)
    }
}

/// The page alphabet (§6.1). The six top-bar destinations are listed in [`crate::tabs::TABS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppArg {
    /// A row per browsable catalog of the installed addons.
    Home,
    /// The query field and one result row per searchable catalog.
    Search,
    /// The installed addons' `tv` and `channel` catalogs, by genre.
    LiveTv,
    /// The user's Stremio library. Until sign-in lands it says how to get one.
    Library,
    /// The installed addons.
    Addons,
    /// The Settings root.
    Settings,
    /// A page of the Settings family.
    SettingsPage(SettingsPage),
    /// One catalog as a full paginated grid.
    Catalog(CatalogRef),
    /// A popover menu over the page that presented it.
    Menu(MenuArg),
    /// One item: its episodes (a series) or its streams per addon (anything else).
    Detail(ItemRef),
    /// One episode's streams per addon.
    Streams(VideoRef),
    /// The playing video, on the hardware plane. The title is server text.
    Player(String),
}

impl ScreenArg for AppArg {
    /// The six top-bar destinations wear the shared bar; everything pushed over them does not.
    fn chrome(&self) -> Chrome {
        if crate::tabs::Tab::of_arg(self).is_some() {
            Chrome::TabBar
        } else {
            Chrome::None
        }
    }
    fn id(&self) -> ScreenId {
        ScreenId(match self {
            AppArg::Home => 1,
            AppArg::Detail(_) => 2,
            AppArg::Streams(_) => 3,
            AppArg::Player(_) => 4,
            AppArg::Search => 5,
            AppArg::LiveTv => 6,
            AppArg::Library => 7,
            AppArg::Addons => 8,
            AppArg::Settings => 9,
            AppArg::SettingsPage(_) => 10,
            AppArg::Catalog(_) => 11,
            AppArg::Menu(_) => 12,
        })
    }
    fn title(&self) -> Option<&str> {
        None
    }
    fn same_instance(&self, other: &Self) -> bool {
        self == other
    }
}

impl LogicalState for AppArg {
    fn write(&self, c: &mut Canon) {
        match self {
            AppArg::Home => {
                c.u32(0);
            }
            AppArg::Detail(i) => {
                c.u32(1).str(&i.kind).str(&i.id).str(&i.name);
            }
            AppArg::Streams(v) => {
                c.u32(2).str(&v.kind).str(&v.video_id).str(&v.show).str(&v.title);
            }
            AppArg::Player(t) => {
                c.u32(3).str(t);
            }
            AppArg::Search => {
                c.u32(4);
            }
            AppArg::LiveTv => {
                c.u32(5);
            }
            AppArg::Library => {
                c.u32(6);
            }
            AppArg::Addons => {
                c.u32(7);
            }
            AppArg::Settings => {
                c.u32(8);
            }
            AppArg::SettingsPage(page) => {
                c.u32(9).u32(page.ordinal());
            }
            AppArg::Catalog(r) => {
                c.u32(10).str(&r.title);
                r.key.write(c);
            }
            AppArg::Menu(m) => {
                c.u32(11).u32(m.host.0);
                m.reply.write_canon(c);
                c.option(m.focus, |c, k| {
                    c.u32(k.entry.0).u32(k.elem);
                });
                for v in m.anchor {
                    c.u32(v);
                }
                match &m.kind {
                    MenuKind::Item(i) => {
                        c.u32(0).str(&i.kind).str(&i.id).str(&i.name);
                    }
                    MenuKind::Choice { title, options, current, target } => {
                        c.u32(1).str(title).u32(*current as u32).u32(*target as u32).seq(options.len());
                        for o in options {
                            c.str(o);
                        }
                    }
                }
            }
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(match self {
            AppArg::Home => word::HOME,
            AppArg::Search => word::SEARCH,
            AppArg::LiveTv => word::LIVE_TV,
            AppArg::Library => word::LIBRARY,
            AppArg::Addons => word::ADDONS,
            AppArg::Settings => word::SETTINGS,
            AppArg::SettingsPage(_) => word::SETTINGS_PAGE,
            AppArg::Catalog(_) => word::CATALOG,
            AppArg::Menu(_) => word::MENU,
            AppArg::Detail(_) => word::DETAIL,
            AppArg::Streams(_) => word::STREAMS,
            AppArg::Player(_) => word::PLAYER,
        });
    }
}

/// A Home page's word to the catalog store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogCmd {
    /// The rows on screen, as indices into the store's row list (viewport-first loading).
    View(std::ops::Range<usize>),
    /// A shelf reached the end of what is loaded.
    More(RowId),
    /// A failed row's read-out was pressed.
    Retry(RowId),
    /// The failed install list's read-out was pressed.
    RetryAddons,
}

/// A Detail or Streams page's word to the detail store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailCmd {
    OpenMeta(MetaKey),
    CloseMeta(MetaKey),
    OpenStreams(StreamKey),
    CloseStreams(StreamKey),
}

/// The Search page's word to the search store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchCmd {
    /// The field's text changed: searched once typing pauses (`stores::search::DEBOUNCE_MS`).
    Type(String),
    /// Search for this now (OK on the field, a remembered term picked).
    Submit(String),
    /// Switch an addon in or out of the search (its manifest id). Persisted.
    ToggleAddon(String),
    /// Ask a failed row again.
    Retry(usize),
    /// Keep this term on the recent-searches list. Persisted.
    Remember(String),
    /// Forget every recent search. Persisted.
    ClearRecents,
}

/// A grid page's word to the grid store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridCmd {
    Open(GridKey),
    Close(GridKey),
    /// The grid reached the end of what is loaded.
    More(GridKey),
    /// A failed page was pressed.
    Retry(GridKey),
}

/// A Settings page's word to the settings store. Every change is persisted by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsCmd {
    /// Set `pref` to rung `index` of its ladder.
    Set(Pref, u8),
}

/// A stream to play: an HTTP(S) URL and the request headers its addon names.
#[derive(Clone, PartialEq, Eq)]
pub struct PlayRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The player page's title. Server text.
    pub title: String,
}

impl std::fmt::Debug for PlayRequest {
    /// Never the URL or a header value: an addon may put a credential in either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayRequest").field("headers", &self.headers.len()).finish()
    }
}

/// What the player page asks of the playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerCmd {
    /// Pause a playing stream, resume a paused one.
    TogglePause,
    /// End the playback (the page is leaving).
    Stop,
}

/// The effects a screen may ask of the application (executed by the rig, `app::bridge`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppFx {
    Catalog(CatalogCmd),
    Detail(DetailCmd),
    Search(SearchCmd),
    Grid(GridCmd),
    Settings(SettingsCmd),
    /// Start a playback; the application pushes the player page when the start is accepted.
    Play(PlayRequest),
    Player(PlayerCmd),
}

/// The application's payloads to a screen. Every store landing reaches a screen as its store's
/// `StoreChanged` notice; a message is a word from one page to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppMsg {
    /// A popover [`MenuKind::Choice`] was picked: delivered to the page that presented it.
    Chose(ChoiceTarget, usize),
}

/// What the player page shows: the playback's phase, as the application reads it off the media
/// layer once a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPhase {
    /// Nothing is playing.
    Idle,
    /// Connecting, buffering, seeking: `caption` says which.
    Working,
    Playing,
    Paused,
    /// The playback failed: `caption` is the verdict, `reason` the line under it.
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct PlaybackView {
    pub phase: PlayPhase,
    pub caption: &'static CStr,
    pub reason: &'static CStr,
}

impl PlaybackView {
    pub const IDLE: PlaybackView = PlaybackView { phase: PlayPhase::Idle, caption: c"", reason: c"" };
}

impl PartialEq for PlaybackView {
    fn eq(&self, other: &Self) -> bool {
        self.phase == other.phase && self.caption == other.caption && self.reason == other.reason
    }
}

/// What the pages know of the account. Signed out until the sign-in work lands; a page that
/// offers an account action (the item menu's Add to library) reads it here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionView {
    pub signed_in: bool,
}

/// The read side: every store's published state, borrowed for one context.
#[derive(Clone, Copy)]
pub struct Views<'a> {
    pub catalog: &'a CatalogStore,
    pub detail: &'a DetailStore,
    pub playback: &'a PlaybackView,
    pub search: &'a SearchStore,
    pub grid: &'a GridStore,
    pub settings: &'a SettingsStore,
    pub session: SessionView,
}

impl<'a> Views<'a> {
    /// **The installed addons every page plans against** — Home's rows come from them, Search
    /// asks their searchable catalogs, Live TV lists their `tv`/`channel` catalogs. ONE accessor,
    /// so the sign-in work switches every page to the user's own collection by changing what this
    /// answers (the application's `Bridge::addons` is its rig-side twin).
    pub fn addons(&self) -> &'a [sn_stremio::manifest::Addon] {
        self.catalog.addons()
    }
}

/// No recorded initial conditions yet (spec §5.3).
#[derive(Default)]
pub struct NoInit;

impl LogicalState for NoInit {
    fn write(&self, _w: &mut Canon) {}
    fn probe(&self, out: &mut String) {
        out.push_str("init");
    }
}

/// The one `mount` match.
#[derive(Default)]
pub struct AppMounter;

impl Mounter<Bundle> for AppMounter {
    fn mount(
        &mut self,
        id: InstanceId,
        arg: &AppArg,
        _ret: &ReturnState<u32>,
        cx: &Cx<'_, Bundle>,
        _fx: &mut Effects<'_, Bundle>,
    ) -> Box<dyn Screen<Bundle>> {
        let entry = match cx.owner {
            InputOwner::Entry(e) => e,
            InputOwner::System(_) => sn_machine::machine::EntryId(0),
        };
        match arg {
            AppArg::Home => Box::new(crate::home::HomeScreen::new(entry)),
            AppArg::Search => Box::new(crate::search::SearchScreen::new(entry, id, cx.views.search)),
            AppArg::LiveTv => Box::new(crate::livetv::LiveTvScreen::new(entry)),
            AppArg::Library => Box::new(crate::library::LibraryScreen::new(entry)),
            AppArg::Addons => Box::new(crate::addons::AddonsScreen::new(entry)),
            AppArg::Settings => Box::new(crate::settings::SettingsScreen::root(entry)),
            AppArg::SettingsPage(page) => Box::new(crate::settings::SettingsScreen::page(entry, *page)),
            AppArg::Catalog(r) => Box::new(crate::grid::GridScreen::new(entry, r.clone())),
            AppArg::Menu(m) => Box::new(crate::menu::MenuScreen::new(entry, m.clone(), cx.views.session)),
            AppArg::Detail(item) => Box::new(crate::detail::DetailScreen::item(entry, item.clone())),
            AppArg::Streams(video) => Box::new(crate::detail::DetailScreen::video(entry, video.clone())),
            AppArg::Player(title) => Box::new(crate::player::PlayerScreen::new(entry, title.clone())),
        }
    }
}

/// The localized label of a Stremio type, or `None` for a type the app has no word for (the
/// type is then shown as the addon sent it).
pub fn kind_label(kind: &str) -> Option<&'static str> {
    use sn_platform::i18n::msg;
    Some(match kind {
        "movie" => msg::app_kind_movie(),
        "series" => msg::app_kind_series(),
        "channel" => msg::app_kind_channel(),
        "tv" => msg::app_kind_tv(),
        _ => return None,
    })
}

/// **A page whose content landed after its own `Enter`** seats its focus once that content is
/// there. A page mounted on nothing selectable (Home while the install list loads, a Detail page
/// whose streams are still out) gets an `Enter` the engine cannot seat, and the engine leaves the
/// scope empty for the page to fill on its first landing (`FocusEngine::enter`'s empty-page
/// arm). This asks the engine for the seat by re-entering the page's own instance with `target`:
/// the dispatcher answers it only while the page owns input, so a page under another one asks
/// harmlessly.
pub struct FirstSeat {
    /// Ticks until the next ask; 0 asks now.
    wait: u32,
}

/// How many ticks an unanswered ask waits before asking again: the seat the engine could not take
/// (the target group laid out a frame later, the page did not own input) is retried, not dropped.
const RESEAT_TICKS: u32 = 30;

impl FirstSeat {
    pub const fn new() -> Self {
        FirstSeat { wait: 0 }
    }

    /// Call on every tick with whether the page holds focus and, when it has something to seat,
    /// the target. Asks at once, then every [`RESEAT_TICKS`] while the focus stays empty.
    pub fn tick(&mut self, focused: bool, target: Option<sn_ui::screen::FocusTarget<u32>>, fx: &mut Effects<'_, Bundle>) {
        if focused {
            self.wait = 0;
            return;
        }
        let Some(target) = target else { return };
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        self.wait = RESEAT_TICKS;
        reseat(target, fx);
    }
}

impl Default for FirstSeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Move the engine's focus on the page that emits this to `target` (a re-`Enter` of its own
/// instance, answered while it owns input).
pub fn reseat(target: sn_ui::screen::FocusTarget<u32>, fx: &mut Effects<'_, Bundle>) {
    let me = fx.from();
    fx.push(sn_machine::machine::Fx::Deliver(
        me,
        sn_machine::machine::Delivery::Screen(sn_ui::screen::ScreenEvent::Enter(sn_ui::screen::Enter::Fresh { focus: target })),
    ));
}
