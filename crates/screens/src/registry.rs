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

use crate::stores::account::{AccountCmd, AccountStore};
use crate::stores::addons::AddonsStore;
use crate::stores::catalog::{CatalogStore, RowId};
use crate::stores::detail::{DetailStore, MetaKey, StreamKey};
use crate::stores::grid::{GridKey, GridStore};
use crate::stores::library::LibraryStore;
use crate::stores::search::SearchStore;
use crate::stores::settings::{LangList, LangOp, Pref, SettingsStore};

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
    pub const DISCOVER: &str = "discover";
    pub const LIBRARY: &str = "library";
    pub const ADDONS: &str = "addons";
    pub const SETTINGS: &str = "settings";
    pub const SETTINGS_PAGE: &str = "settings-page";
    pub const CATALOG: &str = "catalog";
    pub const MENU: &str = "menu";
    pub const DETAIL: &str = "detail";
    pub const STREAMS: &str = "streams";
    pub const PLAYER: &str = "player";
    pub const SIGN_IN: &str = "signin";
    pub const ADDON: &str = "addon";
    pub const RESUME: &str = "resume";
    pub const CONTINUE: &str = "continue";
}

/// An item a catalog showed, as a Detail page opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRef {
    /// The Stremio type (`movie`, `series`, `tv`, `channel`, …).
    pub kind: String,
    pub id: String,
    /// The catalog's own name for it, shown before the meta lands. Server text.
    pub name: String,
    /// The page that opened it, by name: the Detail page's crumb (BACK returns there). Empty is
    /// Home.
    pub from: String,
}

/// Discover opened on one genre (a Detail page's genre link): the type, the genre, and the
/// catalog the link names when it names one (`addon_base` is `sn_stremio::manifest::base_of` of
/// its transport URL); with none, the first catalog of the type that offers the genre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenreRef {
    pub kind: String,
    /// `None`: the catalog unfiltered.
    pub genre: Option<String>,
    pub addon_base: Option<String>,
    pub catalog: Option<String>,
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
    /// Where a play from this page starts, in milliseconds (a Continue Watching resume); 0 from
    /// the beginning.
    pub resume_ms: u64,
    /// The work the video belongs to (a series' id): the library item a play records its
    /// progress on, and the meta the player reads the next episode from.
    pub item_id: String,
    /// The page that opened it, by name, when that is not the show's page (Continue Watching):
    /// the crumb. Empty is the show.
    pub from: String,
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
    /// Edit one ordered language list (add, remove, reorder).
    Languages(LangList),
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
            SettingsPage::Languages(l) => 0x30 + l as u32,
        }
    }
}

/// A popover menu's content (`crate::menu`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuKind {
    /// A card's item menu: Open, Add to / Remove from library (`LibraryCmd::Add`/`Remove`;
    /// disabled while signed out) and, for a title in Continue Watching, Remove from Continue
    /// Watching (`LibraryCmd::Rewind`). `poster` is the
    /// card's artwork, which a new library entry carries.
    Item { item: ItemRef, poster: Option<String> },
    /// Pick one of `options` (the current one checked); the pick goes back to the presenting page
    /// as [`AppMsg::Chose`] with `target`. Options are server text (catalog names, genres).
    Choice { title: String, options: Vec<String>, current: usize, target: ChoiceTarget },
    /// A list of labelled rows with a trailing value each (the player's `…` menu, the subtitle delay
    /// panel); OK on a row goes back as [`AppMsg::Chose`] with `target` and the row's index.
    /// Labels are app text; a value is server text when its row says so.
    List { title: String, rows: Vec<ListRow>, target: ChoiceTarget },
    /// The player's subtitle menu: languages on the left, the chosen language's subtitles on the
    /// right (`crate::player::subtitle_menu`). Its rows are read live off the playback, so a late
    /// addon answer lands in the open menu; a pick asks the playback itself.
    Subtitles,
    /// The account pill's menu (`crate::tabs::open_account_menu`): Settings, Addons, the account
    /// line and Sign out — confirmed in place — or Sign in. The address is read off
    /// `Views::account` when the menu mounts, never carried in the argument (which is logged).
    Account,
    /// One of the player's own panels over the video (`crate::player`): the Episodes panel, the
    /// show info.
    Player(PlayerPanel),
}

/// The player's panels ([`MenuKind::Player`]). Each reads the show's meta live off the detail
/// store (the player holds it open), so a meta landing after the panel opened fills it in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerPanel {
    /// The show's episodes, a season dropdown above them (`crate::player::episodes`), opened on
    /// the season of `video_id` (what plays), which is marked *Now playing*. A pick goes back as
    /// [`AppMsg::Chose`] with [`ChoiceTarget::PlayerEpisode`] and the episode's index in the
    /// meta's videos.
    Episodes { meta: MetaKey, video_id: String },
    /// What plays, as the meta tells it (`crate::player::show_info`): `title` is the player's own
    /// title until the meta lands, `tech` the one-line technical summary under it.
    ShowInfo { meta: Option<MetaKey>, video_id: String, title: String, tech: String },
}

/// One row of a [`MenuKind::List`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub label: String,
    pub value: String,
    /// The value is the stream's or the server's text (a codec, a track name), not the app's.
    pub server_value: bool,
    /// The row opens another list (a chevron) rather than acting.
    pub opens: bool,
    /// A multi-select's row: the id OK switches and whether it is on (a check mark, no value).
    pub switch: Option<(String, bool)>,
}

/// Which control a [`MenuKind::Choice`] answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceTarget {
    /// The player's audio tracks.
    PlayerAudio,
    /// The player's `…` menu.
    PlayerMore,
    /// The subtitle size ladder, from the player.
    PlayerSubtitleSize,
    /// The subtitle position ladder, from the player.
    PlayerSubtitlePosition,
    /// The Skip intros ladder ([`crate::stores::settings::Pref::SkipIntros`]).
    PlayerSkipIntros,
    /// The Dolby Vision Profile 7 ladder ([`crate::stores::settings::Pref::DvProfile7`]).
    PlayerDvProfile7,
    /// The player's aspect modes (`PlayerCmd::Aspect`).
    PlayerAspect,
    /// Discover's type dropdown (its sub-tabs), `crate::dropdown`.
    DiscoverType,
    /// Discover's catalog dropdown.
    DiscoverCatalog,
    /// Discover's genre dropdown.
    DiscoverGenre,
    /// Home's type dropdown.
    HomeType,
    /// The Library's type dropdown.
    LibraryType,
    /// The Library's sort dropdown.
    LibrarySort,
    /// Search's addon filter: a multi-select that stays open (`dropdown::stays_open`).
    SearchAddons,
    /// A series' season dropdown on its Detail page.
    DetailSeason,
    /// A stream list's addon dropdown: which addons are asked for streams (a multi-select that
    /// stays open, `dropdown::stream_addon_menu`).
    DetailStreamAddons,
    /// The subtitle delay's steps (a list that stays open, `dropdown::subtitle_delay_menu`).
    PlayerSubtitleDelay,
    /// The audio sync's steps (`crate::dropdown::audio_sync_menu`).
    PlayerAudioSync,
    /// A chosen language's Move up / Move down / Remove, on a Settings language list.
    SettingsLanguage,
    /// An episode picked in the player's Episodes panel (the index is the meta's video's).
    PlayerEpisode,
    /// The season dropdown inside the player's Episodes panel.
    PlayerEpisodeSeason,
}

impl ChoiceTarget {
    /// A page's dropdown (its popover hangs under the button, `dropdown::panel`), not a player menu.
    pub fn drops(self) -> bool {
        matches!(
            self,
            ChoiceTarget::DiscoverType
                | ChoiceTarget::DiscoverCatalog
                | ChoiceTarget::DiscoverGenre
                | ChoiceTarget::HomeType
                | ChoiceTarget::LibraryType
                | ChoiceTarget::LibrarySort
                | ChoiceTarget::SearchAddons
                | ChoiceTarget::DetailSeason
                | ChoiceTarget::DetailStreamAddons
                | ChoiceTarget::PlayerEpisodeSeason
        )
    }
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

/// The page alphabet (§6.1). The five top-bar destinations are listed in [`crate::tabs::TABS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppArg {
    /// A row per browsable catalog of the installed addons.
    Home,
    /// The query field and one result row per searchable catalog.
    Search,
    /// Discover: the addons' catalogs by type (a sub-tab each), catalog and genre.
    Discover,
    /// Discover opened on its Live TV sub-tab (the `tv` and `channel` catalogs).
    LiveTv,
    /// Discover pushed over a Detail page by a genre link, on that genre: BACK returns to the page.
    DiscoverAt(GenreRef),
    /// Search pushed over a Detail page by a cast or director link, its query filled and run:
    /// BACK returns to the page.
    SearchFor(String),
    /// The account's library, by type (signed out: how to get one).
    Library,
    /// The addons in use, and the account they come from.
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
    /// A series' Detail page opened from one of its episodes' Streams page ("All episodes"): the
    /// show is the video's `item_id`, its list opens on the video's season, and its crumb is the
    /// episode page BACK returns to.
    Overview(VideoRef),
    /// The playing video, on the hardware plane.
    Player(PlayerArg),
    /// Sign in to a Stremio account (the device link, or email and password).
    SignIn,
    /// One addon: what it serves, its catalogs, its search switch. The addon's manifest id.
    Addon(String),
    /// A Continue Watching item with no recorded video: its meta is read, the first episode
    /// chosen, and that episode's streams shown.
    Resume(ItemRef),
    /// Every Continue Watching item, as a grid or a list (Home's Continue Watching heading).
    Continue,
}

impl ScreenArg for AppArg {
    /// The five top-bar destinations wear the shared bar; everything pushed over them does not.
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
            AppArg::SignIn => 13,
            AppArg::Addon(_) => 14,
            AppArg::Resume(_) => 15,
            AppArg::Discover => 16,
            AppArg::Overview(_) => 17,
            AppArg::Continue => 18,
            AppArg::DiscoverAt(_) => 19,
            AppArg::SearchFor(_) => 20,
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
                c.u32(2).str(&v.kind).str(&v.video_id).str(&v.show).str(&v.title).u64(v.resume_ms).str(&v.item_id);
            }
            AppArg::Player(a) => {
                c.u32(3).str(&a.title);
                c.option(a.item.as_ref(), |c, i| {
                    c.str(&i.kind).str(&i.id).str(&i.name).str(&i.video_id).str(&i.episode);
                });
            }
            AppArg::SignIn => {
                c.u32(12);
            }
            AppArg::Addon(id) => {
                c.u32(13).str(id);
            }
            AppArg::Resume(i) => {
                c.u32(14).str(&i.kind).str(&i.id).str(&i.name);
            }
            AppArg::Overview(v) => {
                c.u32(16).str(&v.kind).str(&v.video_id).str(&v.show).str(&v.title).u64(v.resume_ms).str(&v.item_id);
            }
            AppArg::Search => {
                c.u32(4);
            }
            AppArg::LiveTv => {
                c.u32(5);
            }
            AppArg::Discover => {
                c.u32(15);
            }
            AppArg::Continue => {
                c.u32(17);
            }
            AppArg::DiscoverAt(g) => {
                c.u32(18).str(&g.kind);
                for v in [&g.genre, &g.addon_base, &g.catalog] {
                    c.option(v.as_ref(), |c, v| {
                        c.str(v);
                    });
                }
            }
            AppArg::SearchFor(q) => {
                c.u32(19).str(q);
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
                    MenuKind::Item { item: i, poster } => {
                        c.u32(0).str(&i.kind).str(&i.id).str(&i.name);
                        c.option(poster.as_ref(), |c, p| {
                            c.str(p);
                        });
                    }
                    MenuKind::Choice { title, options, current, target } => {
                        c.u32(1).str(title).u32(*current as u32).u32(*target as u32).seq(options.len());
                        for o in options {
                            c.str(o);
                        }
                    }
                    MenuKind::List { title, rows, target } => {
                        c.u32(2).str(title).u32(*target as u32).seq(rows.len());
                        for r in rows {
                            c.str(&r.label).str(&r.value).bool(r.server_value).bool(r.opens);
                        }
                    }
                    MenuKind::Subtitles => {
                        c.u32(3);
                    }
                    MenuKind::Account => {
                        c.u32(4);
                    }
                    MenuKind::Player(PlayerPanel::Episodes { meta, video_id }) => {
                        c.u32(5).str(&meta.kind).str(&meta.id).str(video_id);
                    }
                    MenuKind::Player(PlayerPanel::ShowInfo { meta, video_id, title, tech }) => {
                        c.u32(6).str(video_id).str(title).str(tech);
                        c.option(meta.as_ref(), |c, m| {
                            c.str(&m.kind).str(&m.id);
                        });
                    }
                }
            }
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(match self {
            AppArg::Home => word::HOME,
            AppArg::Search | AppArg::SearchFor(_) => word::SEARCH,
            AppArg::Discover | AppArg::LiveTv | AppArg::DiscoverAt(_) => word::DISCOVER,
            AppArg::Library => word::LIBRARY,
            AppArg::Addons => word::ADDONS,
            AppArg::Settings => word::SETTINGS,
            AppArg::SettingsPage(_) => word::SETTINGS_PAGE,
            AppArg::Catalog(_) => word::CATALOG,
            AppArg::Menu(_) => word::MENU,
            AppArg::Detail(_) | AppArg::Overview(_) => word::DETAIL,
            AppArg::Streams(_) => word::STREAMS,
            AppArg::Player(_) => word::PLAYER,
            AppArg::SignIn => word::SIGN_IN,
            AppArg::Addon(_) => word::ADDON,
            AppArg::Resume(_) => word::RESUME,
            AppArg::Continue => word::CONTINUE,
        });
    }
}

/// A page's word to the addons store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddonsCmd {
    /// Switch search off (or on again) for this addon id.
    ToggleSearch(String),
    /// Switch stream queries off (or on again) for this addon id (a stream list's addon dropdown).
    ToggleStreams(String),
    /// Ask the account for its collection again.
    Retry,
}

/// A page's word to the library store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryCmd {
    Reload,
    /// *Add to library* (the item menu, the Detail page's button).
    Add(sn_stremio::library::NewEntry),
    /// *Remove from library*: the item's id.
    Remove(String),
    /// The item menu's *Remove from Continue Watching* (stremio-core's `RewindLibraryItem`).
    Rewind(String),
    /// *Mark as watched / unwatched* the entry's item, a video or a season of it (the edit is a
    /// watched mark: `Edit::MarkWatched` / `Edit::MarkVideos`; `LibraryStore::mark`).
    Mark(sn_stremio::library::NewEntry, sn_stremio::library::Edit),
}

/// A Home page's word to the catalog store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogCmd {
    /// The rows on screen, as positions among the rows of the type Home shows (viewport-first
    /// loading; a row the type filter hides is never asked for).
    View(crate::stores::catalog::RowWindow),
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
    /// Show a page's items as a list (`true`) or as posters.
    List(crate::stores::settings::ViewPage, bool),
    /// Edit a language list.
    Languages(LangList, LangOp),
    /// The Library page's order (`sn_stremio::library::Sort`).
    LibrarySort(sn_stremio::library::Sort),
}

/// A stream to play: an HTTP(S) URL and the request headers its addon names.
#[derive(Clone, PartialEq, Eq)]
pub struct PlayRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The player page's title. Server text.
    pub title: String,
    /// Where to start, in milliseconds (a resume); 0 from the beginning.
    pub start_ms: u64,
    /// The addon that offered the stream, by its name (Stats for nerds' Source row). Server text.
    pub addon: String,
    /// What is playing, when the page that played it knows (a catalog item, an episode).
    pub item: Option<PlayItem>,
    /// What the stream's addon said about the file: what subtitles addons are asked by.
    pub hints: sn_stremio::subtitles::FileHints,
}

/// What a playback is OF: the player page's title lines and next episode, and the library item
/// its progress is recorded on. Server text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayItem {
    /// The work (a series' id for an episode) and its type, as the library and the meta name it.
    pub kind: String,
    pub id: String,
    /// The work's name (the show's, for an episode).
    pub name: String,
    /// The video playing: an episode's id, a movie's own id.
    pub video_id: String,
    /// The episode's own line (`S1 E5 Title`); empty for anything that is not an episode.
    pub episode: String,
    /// The stream that plays, when a stream list picked it: what the next episode's same release
    /// is looked up by (`sn_stremio::stream_facts::same_release`).
    pub release: Option<sn_stremio::stream_facts::Release>,
}

/// The player page's argument: its title and what is playing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerArg {
    /// Server text.
    pub title: String,
    pub item: Option<PlayItem>,
    /// The addon whose stream plays (its name; server text), for the show info's summary line.
    pub addon: String,
    /// Where the playback started when it resumed a saved position (0: from the beginning): the
    /// page then offers *Start over* for a few seconds.
    pub resumed_ms: u64,
}

impl PlayerArg {
    pub fn of(req: &PlayRequest) -> Self {
        PlayerArg { title: req.title.clone(), item: req.item.clone(), addon: req.addon.clone(), resumed_ms: req.start_ms }
    }
}

impl std::fmt::Debug for PlayRequest {
    /// Never the URL or a header value: an addon may put a credential in either.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayRequest").field("headers", &self.headers.len()).field("start_ms", &self.start_ms).finish()
    }
}

/// What the player page asks of the playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerCmd {
    /// Pause a playing stream, resume a paused one.
    TogglePause,
    /// Pause (the PAUSE key), or resume (PLAY); a no-op when already so.
    SetPaused(bool),
    /// Seek to this position, in milliseconds.
    Seek(u64),
    /// Skip a segment: seek to this position, landing at or after it, never before.
    SkipTo(u64),
    /// Feed the `n`-th audio track of [`MediaFacts::audio`].
    Audio(usize),
    /// Show the `n`-th subtitle of [`MediaFacts::subs`] then [`MediaFacts::addon_subs`] (an
    /// addon's, past the file's own); `None` turns subtitles off.
    Subtitle(Option<usize>),
    /// Show the addon subtitle [`AddonSubFacts::key`] names: by identity, so an answer landing
    /// between the menu's draw and the press cannot shift the pick onto another.
    AddonSubtitle(u32),
    /// Set the subtitle delay, in milliseconds (positive later, negative earlier).
    SubtitleDelay(i64),
    /// End the playback (the page is leaving).
    Stop,
    /// Place the picture in aspect mode `n` (the media layer's `AspectMode::ALL` order), for
    /// this playback.
    Aspect(u8),
    /// Show or hide the Stats for nerds read-out (the `…` menu's last row, Settings' row).
    ToggleStats,
    /// Subtitles off when on; on when off — the subtitle last shown, else the one the subtitle
    /// languages pick (as if the subtitle mode were On). The player's GREEN key.
    ToggleSubtitles,
    /// Set the audio sync, in ms (positive: the sound later), clamped to the media layer's range;
    /// remembered for the stream source (a live channel, else the item) on this device.
    AudioDelay(i64),
    /// Start the stream that failed again, from where it stopped (the failure read-out's *Try
    /// again*); the page stays.
    Retry,
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
    Account(AccountCmd),
    Addons(AddonsCmd),
    Library(LibraryCmd),
    /// Say this on the television's own toast (app text; a page's passing word, e.g. why a stream
    /// does not play here).
    Toast(String),
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

///
/// `clock` moves every frame and is read, not announced: the store's change notice
/// (`StoreId::Playback`) is for the phase and the media facts alone.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackView {
    pub phase: PlayPhase,
    pub caption: &'static CStr,
    pub reason: &'static CStr,
    pub clock: PlayClock,
    pub media: MediaFacts,
    /// The aspect mode this playback is placed in (an index of `PlayerCmd::Aspect`'s list).
    pub aspect: u8,
    /// The Stats for nerds read-out is on.
    pub stats: bool,
    /// The subtitle delay in force, in milliseconds.
    pub sub_delay_ms: i64,
    /// The audio sync in force, in ms (positive: the sound later), `PlayerCmd::AudioDelay`.
    pub audio_delay_ms: i64,
    /// This playback's intro/recap/credits markers (`sn_stremio::skip`), once looked up after
    /// the first picture; none until then, for a live stream, or for an id no service knows.
    pub skips: sn_stremio::skip::Skips,
}

impl PlaybackView {
    pub const IDLE: PlaybackView = PlaybackView {
        phase: PlayPhase::Idle,
        caption: c"",
        reason: c"",
        clock: PlayClock::ZERO,
        media: MediaFacts::NONE,
        aspect: 0,
        stats: false,
        sub_delay_ms: 0,
        audio_delay_ms: 0,
        skips: sn_stremio::skip::Skips::NONE,
    };

    /// The same read-out (phase, caption, reason): what a change of frame is decided on.
    pub fn same_phase(&self, other: &Self) -> bool {
        self.phase == other.phase && self.caption == other.caption && self.reason == other.reason
    }
}

/// Where the playback is, read once a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayClock {
    /// The playhead the viewer sees: a resolving seek's target, else the published position.
    pub pos_ms: u64,
    /// 0 while unknown (a live stream has none).
    pub dur_ms: u64,
    /// A seek is resolving with the picture already up: the transport's own spinner says so.
    pub seeking: bool,
    /// The stream played to its end.
    pub ended: bool,
    /// This playback has shown a picture: a wait now is about the position (the transport's
    /// spinner), not the whole screen (the read-out).
    pub picture: bool,
    /// How full the start buffer is (0..=100) while the playback waits for its first picture,
    /// when the media layer knows (`sn_media::player::prime_fill_pct`); the working read-out's
    /// ring fills with it.
    pub buffer_pct: Option<u8>,
    /// Where the media the pipeline holds runs to (the scrubber's buffered range); 0 unknown.
    pub buffered_ms: u64,
}

impl PlayClock {
    pub const ZERO: PlayClock = PlayClock { pos_ms: 0, dur_ms: 0, seeking: false, ended: false, picture: false, buffer_pct: None, buffered_ms: 0 };
}

/// One audio or subtitle track as the demuxer and the probe name it. Server (file) text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackFacts {
    /// The `language` tag (ISO 639-2, `eng`), `""` when the file has none.
    pub language: String,
    /// The container's own name for the track, `""` when it has none.
    pub name: String,
    /// FFmpeg's codec name (`ac3`, `eac3`, `aac`), `""` when unknown (subtitles).
    pub codec: String,
    /// 0 when unknown.
    pub channels: i64,
    pub atmos: bool,
    /// What an audio track is fed to the television as when the player converts it
    /// (`sn_media::stream::AudioFeed::fed_as`: `"Dolby Digital"`, `"PCM"`, `"Pro Logic II"`),
    /// `""` when it plays as it is.
    pub fed: &'static str,
}

/// One subtitle an addon offers: its language tag, the addon, what the addon names it. Server
/// text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AddonSubFacts {
    pub language: String,
    pub addon: String,
    /// Its identity for this playback ([`PlayerCmd::AddonSubtitle`]): never reused, never moved
    /// by a later landing.
    pub key: u32,
    /// The release or file name the addon gives (`Film.2014.1080p.BluRay`); empty when none.
    pub release: String,
    /// Only the foreign dialogue.
    pub forced: bool,
    /// For the deaf and hard of hearing (SDH).
    pub hearing_impaired: bool,
}

/// The video as the probe read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoFacts {
    pub codec: String,
    pub width: i32,
    pub height: i32,
    /// Frames per second ×1000; 0 when unknown.
    pub fps_milli: i64,
    /// The Dolby Vision profile, when the stream carries one.
    pub dovi_profile: Option<i64>,
}

/// What the media layer knows about the stream playing: its tracks, the ones in use, the video.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaFacts {
    pub audio: Vec<TrackFacts>,
    /// The audio track fed, an index into `audio`; `None` before the stream is read.
    pub audio_sel: Option<usize>,
    pub subs: Vec<TrackFacts>,
    /// The subtitles addons offer for what plays, after the file's own (they land later).
    pub addon_subs: Vec<AddonSubFacts>,
    /// The subtitle shown, an index into `subs` then `addon_subs`; `None` while subtitles are off.
    pub sub_sel: Option<usize>,
    pub video: Option<VideoFacts>,
    /// The file's chapters, in start order (the scrub read-out names the one under its target).
    pub chapters: Vec<ChapterFacts>,
}

impl MediaFacts {
    pub const NONE: MediaFacts = MediaFacts {
        audio: Vec::new(),
        audio_sel: None,
        subs: Vec::new(),
        addon_subs: Vec::new(),
        sub_sel: None,
        video: None,
        chapters: Vec::new(),
    };

    /// The chapter `ms` falls in: the last one starting at or before it, when it has a name.
    pub fn chapter_at(&self, ms: u64) -> Option<&str> {
        self.chapters.iter().rev().find(|c| c.start_ms <= ms).map(|c| c.title.trim()).filter(|t| !t.is_empty())
    }
}

/// One chapter of the file, as its container names it. The title is the file's text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChapterFacts {
    pub start_ms: u64,
    pub title: String,
}

/// Whether an account is signed in, derived from `Views::account`. A page that offers an account
/// action (the item menu's Add to library) reads it here.
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
    pub account: &'a AccountStore,
    /// Which addons are in use (`AddonsStore::in_use`) and search's per-addon switch
    /// (`AddonsStore::filter`).
    pub addons: &'a AddonsStore,
    pub library: &'a LibraryStore,
    pub search: &'a SearchStore,
    pub grid: &'a GridStore,
    pub settings: &'a SettingsStore,
    /// The TV guide: Live TV's now / next.
    pub epg: &'a crate::stores::epg::EpgStore,
    /// Signed in or not (the item menu's Add to library reads it); the account is `account`.
    pub session: SessionView,
}

impl<'a> Views<'a> {
    /// **The addons the app uses**, manifests loaded, in install order — Home's rows, Search's
    /// catalogs and chips, Discover's sub-tabs and catalogs, the grids, a Detail page's meta and
    /// streams all plan against it. Signed in, the account's collection; signed out, the defaults
    /// (`AddonsStore::in_use` says which). The application's `Bridge::addons` is its rig-side twin.
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
            AppArg::Discover => Box::new(crate::discover::DiscoverScreen::new(entry, None)),
            AppArg::LiveTv => Box::new(crate::discover::DiscoverScreen::new(entry, Some(crate::discover::LIVE))),
            AppArg::DiscoverAt(g) => Box::new(crate::discover::DiscoverScreen::at(entry, g.clone())),
            AppArg::SearchFor(q) => Box::new(crate::search::SearchScreen::for_query(entry, id, cx.views.search, q)),
            AppArg::Settings => Box::new(crate::settings::SettingsScreen::root(entry)),
            AppArg::SettingsPage(page) => Box::new(crate::settings::SettingsScreen::page(entry, *page)),
            AppArg::Catalog(r) => Box::new(crate::grid::GridScreen::new(entry, r.clone())),
            AppArg::Menu(m) if m.kind == MenuKind::Subtitles => {
                Box::new(crate::player::subtitle_menu::SubtitleMenu::new(entry, m.clone(), &cx.views))
            }
            AppArg::Menu(MenuArg { kind: MenuKind::Player(PlayerPanel::Episodes { .. }), .. }) => {
                let AppArg::Menu(m) = arg else { unreachable!() };
                Box::new(crate::player::episodes::EpisodesPanel::new(entry, m.clone(), &cx.views))
            }
            AppArg::Menu(MenuArg { kind: MenuKind::Player(PlayerPanel::ShowInfo { .. }), .. }) => {
                let AppArg::Menu(m) = arg else { unreachable!() };
                Box::new(crate::player::show_info::ShowInfoPanel::new(entry, m.clone()))
            }
            AppArg::Menu(m) => {
                let (add, continuing) = match &m.kind {
                    MenuKind::Item { item, .. } => (cx.views.library.add_state(&item.id), cx.views.library.continuing(&item.id)),
                    MenuKind::Choice { .. } | MenuKind::List { .. } | MenuKind::Subtitles | MenuKind::Account | MenuKind::Player(_) => {
                        (crate::stores::library::AddState::Absent, false)
                    }
                };
                let account = cx.views.account.account().map(|a| if a.email.is_empty() { a.user_id.clone() } else { a.email.clone() });
                // a movie's card menu marks it watched (stremio-web's LibItem / Discover item)
                let watched = match &m.kind {
                    MenuKind::Item { item, .. } if item.kind == "movie" => Some(cx.views.library.item(&item.id).is_some_and(|i| i.watched())),
                    _ => None,
                };
                Box::new(
                    crate::menu::MenuScreen::new(entry, m.clone(), cx.views.session, add, continuing)
                        .with_account(account)
                        .with_watched(cx.views.session, continuing, watched),
                )
            }
            AppArg::Detail(item) => Box::new(crate::detail::DetailScreen::item(entry, item.clone())),
            AppArg::Streams(video) => Box::new(crate::detail::DetailScreen::video(entry, video.clone())),
            AppArg::Overview(video) => Box::new(crate::detail::DetailScreen::overview(entry, video.clone())),
            AppArg::Player(arg) => Box::new(crate::player::PlayerScreen::new(entry, arg.clone())),
            AppArg::SignIn => Box::new(crate::signin::SignInScreen::new(entry)),
            AppArg::Addons => Box::new(crate::addons::AddonsScreen::list(entry)),
            AppArg::Addon(id) => Box::new(crate::addons::AddonsScreen::addon(entry, id.clone())),
            AppArg::Library => Box::new(crate::library::LibraryScreen::new(entry)),
            AppArg::Resume(item) => Box::new(crate::detail::DetailScreen::resume(entry, item.clone())),
            AppArg::Continue => Box::new(crate::continue_page::ContinueScreen::new(entry)),
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
        "anime" => msg::app_kind_anime(),
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
