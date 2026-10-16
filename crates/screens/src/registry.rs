//! **The application bundle** (restructure spec §3.1): the one [`Host`] the dispatcher, the
//! containers and every screen here are compiled against — the screen argument ([`AppArg`]), the
//! effects a screen may ask of the application ([`AppFx`]), the read side ([`Views`]) — and the
//! one `mount` match ([`AppMounter`]).
//!
//! A new screen touches its own module, a variant of [`AppArg`] and an arm of
//! [`AppMounter::mount`] here, and nothing else in this crate.

use std::ffi::CStr;

use sn_machine::machine::{Canon, Chrome, Cx, Effects, Host, InputOwner, InstanceId, LogicalState, ScreenArg, ScreenId};
use sn_ui::screen::{Mounter, ReturnState, Screen};

use crate::stores::catalog::{CatalogStore, RowId};
use crate::stores::detail::{DetailStore, MetaKey, StreamKey};

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

/// The page alphabet (§6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppArg {
    /// A row per browsable catalog of the installed addons.
    Home,
    /// One item: its episodes (a series) or its streams per addon (anything else).
    Detail(ItemRef),
    /// One episode's streams per addon.
    Streams(VideoRef),
    /// The playing video, on the hardware plane. The title is server text.
    Player(String),
}

impl ScreenArg for AppArg {
    fn chrome(&self) -> Chrome {
        Chrome::None
    }
    fn id(&self) -> ScreenId {
        ScreenId(match self {
            AppArg::Home => 1,
            AppArg::Detail(_) => 2,
            AppArg::Streams(_) => 3,
            AppArg::Player(_) => 4,
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
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(match self {
            AppArg::Home => word::HOME,
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
    /// Start a playback; the application pushes the player page when the start is accepted.
    Play(PlayRequest),
    Player(PlayerCmd),
}

/// The application's payloads to a screen. None yet: every landing reaches a screen as its
/// store's `StoreChanged` notice, never as a payload.
pub enum AppMsg {}

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

/// The read side: every store's published state, borrowed for one context.
#[derive(Clone, Copy)]
pub struct Views<'a> {
    pub catalog: &'a CatalogStore,
    pub detail: &'a DetailStore,
    pub playback: &'a PlaybackView,
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
        _id: InstanceId,
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
        let me = fx.from();
        fx.push(sn_machine::machine::Fx::Deliver(
            me,
            sn_machine::machine::Delivery::Screen(sn_ui::screen::ScreenEvent::Enter(sn_ui::screen::Enter::Fresh { focus: target })),
        ));
    }
}

impl Default for FirstSeat {
    fn default() -> Self {
        Self::new()
    }
}
