//! **The shared top bar's owner** — the labels, the strip's motion and the control geometry the
//! container focuses, published once a frame for whichever tab page is up. The application's
//! rig (`app::bridge`) and the host test rig both own one, so the bar a test navigates is the
//! bar the television draws.
//!
//! The pieces are `sn_ui`'s (PlxNative's top bar): `StripRender` draws the track, the pills and
//! the travelling capsules, `tab_members` publishes each pill's drawn and resting rect under its
//! stable control id, and the container (`sn_ui::containers::tabs`) composes those members above
//! the page's own groups as the `STRIP` row. This module only says WHICH destinations there are
//! ([`crate::tabs::TABS`]), which one is selected, and when the bar is up at all (a page whose
//! argument wears `Chrome::TabBar`).
//!
//! **The account pill** (`sn_ui::widgets::profile_chip_with`, the bar's other kind of stop) leads
//! the row at the left margin, before Home ([`crate::tabs::ACCOUNT_ELEM`]): the account's initial on
//! a face picked from `theme::AVATAR_FILLS` by a hash of the account id, and beside it the email's
//! local part — or, signed out, the person glyph and *Sign in*. The name stands open at rest when
//! the track leaves room for it, else it unfurls while the pill is focused. The full email is never
//! drawn here (only the account menu's account line shows it).
//!
//! Order per frame: [`ChromeBar::capture`] before the dispatcher's frame (so the frame's input
//! composes the bar this frame shows), [`ChromeBar::update`] after it (the strip's springs and the
//! members at their new scroll), [`ChromeBar::draw`] from `Rig::draw_chrome`.

use std::ffi::CString;

use sn_machine::machine::{FocusKey, Measure};
use sn_platform::i18n::msg;
use sn_ui::containers::tabs::StripMember;
use sn_ui::dispatch::Dispatcher;
use sn_ui::frame::glass::GlassPlan;
use sn_ui::screen::{NavPresentation, ScreenArg};
use sn_ui::theme;
use sn_ui::widgets::{self, ProfileChipRead, StripRender, TabLabels, TopFocus};

use crate::registry::{AppArg, Bundle};
use crate::stores::account::AccountStore;
use crate::tabs::{current_tab, Tab, ACCOUNT_ELEM, TABS};

/// The account pill's face, measured when what it says changes.
struct AccountFace {
    /// The unelided name and the initial it was measured from.
    said: (String, String),
    initial: CString,
    name: CString,
    name_w: f32,
    fill: [f32; 4],
}

pub struct ChromeBar {
    labels: Vec<String>,
    keys: Vec<u32>,
    widths: Vec<f32>,
    strip: StripRender,
    /// The selected destination (Settings: the account pill is the current page's stop).
    selected: Tab,
    /// What in the bar holds the remote, as of the last update.
    focus: TopFocus,
    /// Whether the page up wears the bar this frame.
    shown: bool,
    account: AccountFace,
}

impl Default for ChromeBar {
    fn default() -> Self {
        Self::new()
    }
}

/// The short name the account pill shows: the email's local part, never the address.
pub fn short_name(email: &str) -> Option<&str> {
    email.split('@').next().map(str::trim).filter(|s| !s.is_empty())
}

/// The account's avatar face: a stable pick from [`theme::AVATAR_FILLS`] by a hash of its id.
pub fn avatar_fill(user_id: &str) -> [f32; 4] {
    let h = sn_base::sha256::sha256(user_id.as_bytes());
    theme::AVATAR_FILLS[usize::from(h[0]) % theme::AVATAR_FILLS.len()]
}

impl ChromeBar {
    pub fn new() -> Self {
        ChromeBar {
            labels: Vec::new(),
            keys: Vec::new(),
            widths: Vec::new(),
            strip: StripRender::new(),
            selected: Tab::Home,
            focus: TopFocus::Away,
            shown: false,
            account: AccountFace { said: Default::default(), initial: CString::default(), name: CString::default(), name_w: 0.0, fill: theme::CONTROL_IDLE_FILL },
        }
    }

    /// The pills' labels, in bar order. The Search pill's is empty: the strip draws it as the
    /// magnifier.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// The selected destination: the page up, or the one a transition is going to.
    pub fn selected(&self) -> Tab {
        self.selected
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The account pill's initial (empty: the person glyph) and name, as drawn.
    pub fn account_text(&self) -> (&str, &str) {
        (self.account.initial.to_str().unwrap_or(""), self.account.name.to_str().unwrap_or(""))
    }

    /// The account pill's face.
    pub fn account_fill(&self) -> [f32; 4] {
        self.account.fill
    }

    /// The account pill's name stands beside it at rest (there is room before the track).
    pub fn account_rests_open(&self) -> bool {
        widgets::chip_rests_open(&self.widths, self.account.name_w)
    }

    fn vocabulary(&mut self, measure: &dyn Measure) {
        if !self.labels.is_empty() {
            return;
        }
        self.labels = TABS.iter().map(|e| if e.tab == Tab::Search { String::new() } else { e.tab.label().to_string() }).collect();
        self.keys = TABS.iter().map(|e| e.tab.elem()).collect();
        self.widths = widgets::tab_widths(&self.labels, measure);
    }

    /// Rebuild the account pill's face when the account (or the language) changed what it says.
    fn face(&mut self, account: &AccountStore, measure: &dyn Measure) {
        let (initial, label, fill) = match account.account() {
            Some(a) => {
                let name = short_name(&a.email).map_or_else(|| msg::app_addons_account().to_string(), str::to_string);
                let initial = name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
                (initial, name, avatar_fill(&a.user_id))
            }
            None => (String::new(), msg::app_addons_sign_in().to_string(), theme::CONTROL_IDLE_FILL),
        };
        self.account.fill = fill;
        if self.account.said.0 == label && self.account.said.1 == initial {
            return;
        }
        let (initial_c, name, name_w) = widgets::profile_chip_text(&label, &initial, measure);
        self.account = AccountFace { said: (label, initial), initial: initial_c, name, name_w, fill };
    }

    /// What in the bar holds the remote.
    pub fn focus(&self, focus: Option<FocusKey<u32>>) -> TopFocus {
        match focus.map(|f| f.elem) {
            Some(ACCOUNT_ELEM) => TopFocus::Chip,
            Some(elem) => self.keys.iter().position(|k| *k == elem).map_or(TopFocus::Away, TopFocus::Pill),
            None => TopFocus::Away,
        }
    }

    /// The selected pill's index for the strip (-1: none of them, the account pill's page is up).
    fn pill(&self) -> i32 {
        self.selected.index().map_or(-1, |i| i as i32)
    }

    /// The bar's members: the account pill first (LEFT from Home reaches it), then the pills.
    fn publish(&self, d: &mut Dispatcher<Bundle>, focus: TopFocus) {
        d.nav.tabs.strip.clear();
        d.nav.tabs.strip.push(StripMember::new(ACCOUNT_ELEM, widgets::CHIP_FRAME));
        widgets::tab_members(&self.widths, &self.keys, self.pill(), focus, self.strip.scroll_pos(), &mut d.nav.tabs.strip);
    }

    /// Publish the bar for the page being shown (or mounted this frame): its members, and the
    /// selected destination's stop as the focus fallback (never the account pill unless its own
    /// page is up, so the boot's seat and UP from a page land on that page's pill). A page that
    /// does not wear the bar gets none, so its focus can never land on a pill.
    pub fn capture(&mut self, d: &mut Dispatcher<Bundle>, account: &AccountStore, measure: &dyn Measure) {
        self.vocabulary(measure);
        self.face(account, measure);
        let wears = d.top_or_mounting_arg().is_none_or(|a| a.chrome() == sn_machine::machine::Chrome::TabBar);
        self.shown = wears;
        if !wears {
            d.nav.tabs.strip.clear();
            d.nav.tabs.strip_fallback = None;
            return;
        }
        self.selected = selected(d);
        d.nav.tabs.strip_fallback = Some(self.selected.elem());
        let focus = self.focus(d.focus());
        self.publish(d, focus);
    }

    /// Step the strip's scroll, capsules and the tab band's material, and republish the members
    /// at the scroll they moved to.
    pub fn update(&mut self, d: &mut Dispatcher<Bundle>, glass: Option<&mut GlassPlan>, dt: f32) {
        if !self.shown || self.labels.is_empty() {
            return;
        }
        self.selected = selected(d);
        self.focus = self.focus(d.focus());
        let labels = TabLabels { generation: 1, labels: &self.labels };
        self.strip.update(labels, self.pill(), self.focus, dt);
        if let Some(glass) = glass {
            glass.step_tab_band(dt);
        }
        self.publish(d, self.focus);
    }

    /// The account pill as the bar draws it this frame.
    fn chip(&self) -> ProfileChipRead<'_> {
        ProfileChipRead {
            src: 0,
            thumb: "",
            initial: &self.account.initial,
            name: &self.account.name,
            name_w: self.account.name_w,
            fill: self.account.fill,
            rest_open: self.account_rests_open(),
            current: self.selected == Tab::Settings,
            bar_focused: self.focus != TopFocus::Away,
        }
    }

    /// `Rig::draw_chrome`: the bar over a page that wears it, the account pill after the strip (it
    /// wears the material the strip's draw published). Nothing without the application's glass
    /// plan (a host test rig), whose band is the track's material.
    pub fn draw(&mut self, arg: &AppArg, nav: NavPresentation, glass: Option<&mut GlassPlan>) {
        if arg.chrome() != sn_machine::machine::Chrome::TabBar || self.labels.is_empty() {
            return;
        }
        let Some(glass) = glass else { return };
        let p = sn_ui::Painter::root().alpha(nav.chrome_alpha);
        let labels = TabLabels { generation: 1, labels: &self.labels };
        self.strip.draw(labels, p, glass.tab_band_mut());
        let face = glass.tab_band_mut().face();
        widgets::profile_chip_with(p, self.chip(), self.strip.chip_expand_pos(), face);
    }
}

/// The selected destination: the one a transition in flight is going to, else the topmost tab
/// page on the stack.
fn selected(d: &Dispatcher<Bundle>) -> Tab {
    d.nav
        .tabs
        .stack
        .pending_dest()
        .and_then(Tab::of_arg)
        .or_else(|| current_tab(d).map(|(t, _)| t))
        .unwrap_or(Tab::Home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::account::{Account, AccountLanding, Secret, Stored};
    use sn_machine::machine::EntryId;

    fn signed_in(user_id: &str, email: &str) -> AccountStore {
        let mut a = AccountStore::new();
        let account = Account { user_id: user_id.into(), email: email.into(), auth_key: Secret::new("k") };
        let _ = a.land(AccountLanding::Restored(Stored::Session(account)), 0);
        a
    }

    #[test]
    fn the_bar_reports_its_focus_by_destination_identity() {
        let mut bar = ChromeBar::new();
        bar.vocabulary(&sn_ui::fixture::FixtureMeasure);
        assert_eq!(bar.labels().len(), TABS.len());
        assert_eq!(bar.labels().last().map(String::as_str), Some(""), "the Search pill is the magnifier");
        let at = |elem: u32| Some(FocusKey { entry: EntryId(1), elem });
        assert_eq!(bar.focus(at(Tab::Home.elem())), TopFocus::Pill(0));
        assert_eq!(bar.focus(at(Tab::Library.elem())), TopFocus::Pill(2));
        assert_eq!(bar.focus(at(ACCOUNT_ELEM)), TopFocus::Chip, "the account pill is the bar's chip");
        assert_eq!(bar.focus(at(Tab::Settings.elem())), TopFocus::Chip, "…and Settings' stop");
        assert_eq!(bar.focus(Some(FocusKey { entry: EntryId(1), elem: 7 })), TopFocus::Away);
    }

    /// Signed in: the initial and the email's local part, never the address, on a face the
    /// account id picks; signed out: the person glyph and *Sign in* on the idle face.
    #[test]
    fn the_account_pill_shows_the_account_or_sign_in() {
        let m = sn_ui::fixture::FixtureMeasure;
        let mut bar = ChromeBar::new();
        bar.vocabulary(&m);
        bar.face(&AccountStore::signed_out(true), &m);
        assert_eq!(bar.account_text(), ("", msg::app_addons_sign_in()), "signed out: the person glyph and Sign in");
        assert_eq!(bar.account_fill(), theme::CONTROL_IDLE_FILL);
        let store = signed_in("u1", "viewer.name@example.com");
        bar.face(&store, &m);
        assert_eq!(bar.account_text(), ("V", "viewer.name"), "the initial and the email's local part");
        assert!(!bar.account_text().1.contains('@'), "the address is never drawn on the bar");
        assert!(theme::AVATAR_FILLS.contains(&bar.account_fill()));
        assert_eq!(bar.account_fill(), avatar_fill("u1"), "the face is the account id's");
        assert_eq!(avatar_fill("u1"), avatar_fill("u1"), "and stable");
        assert!((0..32).map(|i| avatar_fill(&format!("user{i}"))).collect::<Vec<_>>().windows(2).any(|w| w[0] != w[1]), "accounts differ");
        bar.face(&signed_in("u2", ""), &m);
        assert_eq!(bar.account_text(), ("A", msg::app_addons_account()), "no email: the word Account");
        assert_eq!(short_name("@x"), None);
    }

    /// The account pill's name stands open at rest beside the shipped bar in every language,
    /// signed out (*Sign in*) and for an ordinary name; a bar with no room beside it unfurls the
    /// name on focus instead of reaching into the track.
    #[test]
    fn the_account_pill_fits_beside_the_bar_in_every_shipped_language() {
        use sn_base::fontcov::advances::ShippedMeasure;
        use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            for store in [AccountStore::signed_out(true), signed_in("u1", "alexandra.k@example.com")] {
                let mut bar = ChromeBar::new();
                bar.vocabulary(&ShippedMeasure);
                bar.face(&store, &ShippedMeasure);
                if !bar.account_rests_open() {
                    out.push(format!("{}: {:?} does not fit beside the bar", language.tag(), bar.account_text().1));
                }
            }
        }
        assert!(!widgets::chip_rests_open(&[400.0; 4], 320.0), "a name with no room beside the track unfurls on focus only");
        assert!(out.is_empty(), "{out:#?}");
    }
}
