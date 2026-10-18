//! **The dropdown** — the one selector control a page puts under the top bar: a pill showing the
//! current value with a chevron-down (`Button::trailing_icon(Icon::ChevronDown)`), which OK or a
//! click opens as the shared popover (`crate::menu`, `MenuKind::Choice`) hanging UNDER the button
//! ([`panel`]), the current option checked and focused. A pick comes back to the page as
//! `AppMsg::Chose(target, index)`; BACK closes the popover with nothing changed. Stremio's own
//! Discover row ("Movies ▾  Popular ▾  Genre ▾").
//!
//! - [`Dropdown`] is one single-choice button: its options `(key, label)`, the choice (kept per
//!   page for the session through `type_filter`'s memory, or by its owner), and its one element.
//!   Home's and the Library's type filter, Discover's type, catalog and genre, Detail's seasons.
//! - [`face_w`] / [`draw_face`] / [`row`] / [`group`] are the button and the row a page lays its
//!   dropdowns (and its View toggle, at the right end) out on.
//! - Search's addon filter is the multi-select form ([`addon_menu`]): a `MenuKind::List` that
//!   stays open while OK toggles a row ([`stays_open`]) and re-reads its rows from the store
//!   ([`live`]), with *Select all* and *Select none* first. A stream list's addon dropdown
//!   ([`stream_addon_menu`]) is the same popover: a check per addon asked for streams.
//! - The player's subtitle delay ([`subtitle_delay_menu`]) is the same kind of popover: a list
//!   that stays open while OK steps the delay, its title re-read from the playback.

use std::cell::RefCell;
use std::ffi::CString;

use sn_machine::machine::{Cx, EntryId, FocusKey, GroupId, Measure};
use sn_platform::i18n::msg;
use sn_stremio::search::AddonFilter;
use sn_stremio::streams::StreamGroup;
use sn_ui::consts::{MARGIN_X, SCR_H, SCR_W};
use sn_ui::screen::{AxisMask, DrawFrame, EdgeRule, ElemKind, GroupKind, GroupSpec, Seat};
use sn_ui::table::{MENU_MAX_W, MENU_MIN_W};
use sn_ui::widgets::Button;
use sn_ui::{theme, Env, Painter, Rect, View};

use crate::registry::{Bundle, ChoiceTarget, ListRow, MenuArg, MenuKind, Views};
use crate::type_filter::{self, Page, ALL};

/// A button's height (the top bar's pill).
pub const H: f32 = type_filter::CHIP_H;
/// The selector row's band: the buttons and the air under them.
pub const BAND: f32 = type_filter::BAND;
/// The air between two buttons.
const GAP: f32 = theme::space::MD;
const SIZE: i32 = theme::size::BODY;
/// The widest a button grows; a longer value (a catalog's name) elides inside it.
pub const BUTTON_W_CAP: f32 = 560.0;
/// The air between a button and the popover under it.
const DROP_GAP: f32 = theme::space::SM;
/// The popover's keep-out from the screen's bottom.
const EDGE: f32 = theme::space::XL;

/// A button's width for `label`: the pill with its chevron, at most [`BUTTON_W_CAP`].
pub fn face_w(label: &str, measure: &dyn Measure) -> f32 {
    let c = CString::new(label.replace('\0', "")).unwrap_or_default();
    Button::pill_w_measured(&c, SIZE, false, true, measure).min(BUTTON_W_CAP)
}

/// A button at `rect`: `label` (elided to the pill), the chevron-down after it.
pub fn draw_face(p: Painter, rect: Rect, label: &str, focused: bool, measure: &dyn Measure) {
    let air = Button::pill_w_measured(c"", SIZE, false, true, measure);
    let text = sn_gfx::text::elide_by(label, rect.w - air, false, |t| measure.width_str(t, SIZE, true));
    let c = CString::new(text.replace('\0', "")).unwrap_or_default();
    Button::new(c.as_ptr(), SIZE, rect).trailing_icon(sn_ui::icons::Icon::ChevronDown).focused(focused).draw(&Env::inert(), p);
}

/// The row's rects at `y`: the buttons of `widths` from the left margin, then a `tail` control
/// (the View toggle) on the right margin — after the buttons when they reach that far.
pub fn row(widths: &[f32], tail: Option<f32>, y: f32) -> Vec<Rect> {
    let mut x = MARGIN_X;
    let mut out: Vec<Rect> = widths
        .iter()
        .map(|w| {
            let r = Rect::new(x, y, *w, H);
            x += w + GAP;
            r
        })
        .collect();
    if let Some(w) = tail {
        out.push(Rect::new((SCR_W - MARGIN_X - w).max(x), y, w, H));
    }
    out
}

/// The row's focus group: LEFT/RIGHT along it, UP and DOWN out by geometry.
pub fn group(id: GroupId, extent: Rect, len: usize) -> GroupSpec {
    GroupSpec {
        id,
        kind: GroupKind::Row { wrap: false },
        seat: Seat::First,
        reachable: AxisMask::BOTH,
        edge: [EdgeRule::Geometric, EdgeRule::Geometric, EdgeRule::Stop, EdgeRule::Stop],
        extent,
        len,
        elem: ElemKind::Control,
    }
}

/// The popover `kind`, opened from the button at `rect` (focus returns to it on close).
pub fn open(host: EntryId, cx: &Cx<'_, Bundle>, fx: &mut sn_machine::machine::Effects<'_, Bundle>, rect: Rect, kind: MenuKind) {
    use sn_machine::machine::{Fx, NavOp};
    fx.push(Fx::Nav(NavOp::Present(crate::registry::AppArg::Menu(MenuArg {
        host,
        reply: fx.from(),
        focus: cx.focus.current,
        anchor: MenuArg::anchor_of(rect),
        kind,
    }))));
}

fn target(kind: &MenuKind) -> Option<ChoiceTarget> {
    match kind {
        MenuKind::Choice { target, .. } | MenuKind::List { target, .. } => Some(*target),
        MenuKind::Item { .. } | MenuKind::Subtitles | MenuKind::Account | MenuKind::Player(_) => None,
    }
}

/// Where a dropdown's popover stands (`crate::menu` asks; `None`: not a dropdown's): under its
/// button, left edges aligned and at least as wide, pulled inside the screen; a long list is
/// clipped to the screen's bottom and scrolls inside.
pub fn panel(arg: &MenuArg, content_w: f32, content_h: f32) -> Option<Rect> {
    if !target(&arg.kind)?.drops() {
        return None;
    }
    Some(under(arg.anchor_rect(), content_w, content_h))
}

/// A popover hung under the control at `a`: left edges aligned and at least as wide, pulled inside
/// the screen; a long list is clipped to the screen's bottom (the dropdowns, the account menu).
pub fn under(a: Rect, content_w: f32, content_h: f32) -> Rect {
    let w = content_w.clamp(MENU_MIN_W.max(a.w), MENU_MAX_W);
    let x = a.x.clamp(MARGIN_X, (SCR_W - MARGIN_X - w).max(MARGIN_X));
    let y = a.y + a.h + DROP_GAP;
    let h = content_h.clamp(120.0, (SCR_H - EDGE - y).max(120.0));
    Rect::new(x, y, w, h)
}

/// Does OK on a row of `kind` leave the popover open (a multi-select toggles in place, the
/// subtitle delay steps)?
pub fn stays_open(kind: &MenuKind) -> bool {
    matches!(
        target(kind),
        Some(ChoiceTarget::SearchAddons | ChoiceTarget::DetailStreamAddons | ChoiceTarget::PlayerSubtitleDelay | ChoiceTarget::PlayerAudioSync)
    )
}

/// A popover whose rows follow a store: its rows now (`None`: fixed rows).
pub fn live(kind: &MenuKind, views: &Views<'_>) -> Option<MenuKind> {
    match target(kind)? {
        ChoiceTarget::SearchAddons => Some(addon_menu(views)),
        ChoiceTarget::DetailStreamAddons => Some(stream_addon_live(kind, views.addons.stream_filter())),
        ChoiceTarget::PlayerSubtitleDelay => Some(subtitle_delay_menu(views.playback)),
        ChoiceTarget::PlayerAudioSync => Some(audio_sync_menu(views.playback)),
        _ => None,
    }
}

// --- the player's subtitle delay ----------------------------------------------------------------

/// The subtitle delay's step: a quarter second.
pub const DELAY_STEP_MS: i64 = 250;

/// A delay as the menus write it: `+0.25 s`, `−1.50 s`, `0.00 s`.
pub fn delay_label(ms: i64) -> String {
    let sign = match ms.signum() {
        1 => "+",
        -1 => "\u{2212}",
        _ => "",
    };
    let abs = ms.unsigned_abs();
    msg::app_player_seconds(&format!("{sign}{}.{:02}", abs / 1000, abs % 1000 / 10))
}

/// The subtitle delay's steps, which stay open while OK steps (the player's `tracks::DELAY_ROWS`
/// order): *Earlier* and *Later* by [`DELAY_STEP_MS`], *Reset*; the title says the delay in force.
pub fn subtitle_delay_menu(playback: &crate::registry::PlaybackView) -> MenuKind {
    let row = |label: &str, value: String| ListRow { label: label.to_string(), value, server_value: false, opens: false, switch: None };
    let (step, label) = (DELAY_STEP_MS, delay_label);
    MenuKind::List {
        title: format!("{} \u{b7} {}", msg::app_player_subtitle_delay(), label(playback.sub_delay_ms)),
        rows: vec![
            row(msg::app_player_subtitle_earlier(), label(-step)),
            row(msg::app_player_subtitle_later(), label(step)),
            row(msg::app_player_subtitle_delay_reset(), label(0)),
        ],
        target: ChoiceTarget::PlayerSubtitleDelay,
    }
}

/// The audio sync's step: a tenth of a second.
pub const AUDIO_SYNC_STEP_MS: i64 = 100;

/// **The audio sync's steps** (the audio menu's *Audio sync* row; the subtitle menu's Timing
/// model): *Earlier* and *Later* by [`AUDIO_SYNC_STEP_MS`], *Reset*; the title says the sync in
/// force. It stays open and follows the playback, so the steps can be walked.
pub fn audio_sync_menu(playback: &crate::registry::PlaybackView) -> MenuKind {
    let row = |label: &str, value: String| ListRow { label: label.to_string(), value, server_value: false, opens: false, switch: None };
    MenuKind::List {
        title: format!("{} \u{b7} {}", msg::app_player_audio_sync(), delay_label(playback.audio_delay_ms)),
        rows: vec![
            row(msg::app_player_subtitle_earlier(), delay_label(-AUDIO_SYNC_STEP_MS)),
            row(msg::app_player_subtitle_later(), delay_label(AUDIO_SYNC_STEP_MS)),
            row(msg::app_player_subtitle_delay_reset(), delay_label(0)),
        ],
        target: ChoiceTarget::PlayerAudioSync,
    }
}

/// The audio sync row `i` asks for, from `delay_ms` (`PlayerCmd::AudioDelay`'s value).
pub fn audio_sync_pick(i: usize, delay_ms: i64) -> Option<i64> {
    match i {
        0 => Some(delay_ms - AUDIO_SYNC_STEP_MS),
        1 => Some(delay_ms + AUDIO_SYNC_STEP_MS),
        2 => Some(0),
        _ => None,
    }
}

// --- Search's addon filter ----------------------------------------------------------------------

/// The addons Search can ask (`(id, name)`, a nameless one by its id).
fn searchable<'a>(views: &Views<'a>) -> Vec<(&'a str, &'a str)> {
    sn_stremio::search::searchable_addons(views.addons()).into_iter().map(|(id, name)| (id, if name.is_empty() { id } else { name })).collect()
}

/// The addon filter's button: *All addons*, the one addon on, or "3 of 7 addons".
pub fn addon_summary(views: &Views<'_>) -> String {
    let all = searchable(views);
    let on: Vec<&str> = all.iter().filter(|(id, _)| views.addons.filter().allows(id)).map(|(_, n)| *n).collect();
    match on.len() {
        n if n == all.len() => msg::app_search_addons_all().to_string(),
        1 => on[0].to_string(),
        n => msg::app_search_addons_some(n as i64, all.len() as i64),
    }
}

/// The addon filter's popover: *Select all*, *Select none*, then each addon with On/Off.
pub fn addon_menu(views: &Views<'_>) -> MenuKind {
    let row = |label: &str, value: &str| ListRow { label: label.to_string(), value: value.to_string(), server_value: false, opens: false, switch: None };
    let mut rows = vec![row(msg::app_search_select_all(), ""), row(msg::app_search_select_none(), "")];
    for (id, name) in searchable(views) {
        let on = views.addons.filter().allows(id);
        rows.push(row(name, if on { msg::app_search_addon_on() } else { msg::app_search_addon_off() }));
    }
    MenuKind::List { title: msg::app_search_addons().to_string(), rows, target: ChoiceTarget::SearchAddons }
}

/// The addons row `i` of [`addon_menu`] switches (each id once: a toggle).
pub fn addon_toggles(views: &Views<'_>, i: usize) -> Vec<String> {
    let all = searchable(views);
    let on = |id: &str| views.addons.filter().allows(id);
    match i {
        0 => all.iter().filter(|(id, _)| !on(id)).map(|(id, _)| id.to_string()).collect(),
        1 => all.iter().filter(|(id, _)| on(id)).map(|(id, _)| id.to_string()).collect(),
        i => all.get(i - 2).map(|(id, _)| vec![id.to_string()]).unwrap_or_default(),
    }
}

// --- a stream list's addon dropdown -------------------------------------------------------------

/// The addons a stream list asks (`(id, name)`, a nameless one by its id): its groups, in order.
fn stream_addons(groups: &[StreamGroup]) -> impl Iterator<Item = (&str, &str)> {
    groups.iter().map(|g| (g.addon_id.as_str(), if g.addon_name.is_empty() { g.addon_id.as_str() } else { g.addon_name.as_str() }))
}

/// The stream addon dropdown's button: *All addons*, the one addon asked, or "3 of 7 addons".
pub fn stream_addon_summary(groups: &[StreamGroup], filter: &AddonFilter) -> String {
    let on: Vec<&str> = stream_addons(groups).filter(|(id, _)| filter.allows(id)).map(|(_, n)| n).collect();
    match on.len() {
        n if n == groups.len() => msg::app_search_addons_all().to_string(),
        1 => on[0].to_string(),
        n => msg::app_search_addons_some(n as i64, groups.len() as i64),
    }
}

/// The stream addon dropdown's popover: each addon serving the list, checked while it is asked.
/// Row `n` is group `n` ([`stream_addon_toggles`]).
pub fn stream_addon_menu(groups: &[StreamGroup], filter: &AddonFilter) -> MenuKind {
    stream_addon_rows(stream_addons(groups).map(|(id, name)| (id.to_string(), name.to_string())), filter)
}

fn stream_addon_rows(addons: impl Iterator<Item = (String, String)>, filter: &AddonFilter) -> MenuKind {
    let rows = addons
        .map(|(id, name)| {
            let on = filter.allows(&id);
            ListRow { label: name, value: String::new(), server_value: false, opens: false, switch: Some((id, on)) }
        })
        .collect();
    MenuKind::List { title: msg::app_search_addons().to_string(), rows, target: ChoiceTarget::DetailStreamAddons }
}

/// The open popover's rows re-read against `filter` (the same addons, their checks now).
fn stream_addon_live(kind: &MenuKind, filter: &AddonFilter) -> MenuKind {
    let MenuKind::List { rows, .. } = kind else { return kind.clone() };
    stream_addon_rows(rows.iter().filter_map(|r| r.switch.as_ref().map(|(id, _)| (id.clone(), r.label.clone()))), filter)
}

/// The addon row `n` of [`stream_addon_menu`] switches.
pub fn stream_addon_toggles(groups: &[StreamGroup], n: usize) -> Option<String> {
    groups.get(n).map(|g| g.addon_id.clone())
}

// --- one single-choice dropdown -----------------------------------------------------------------

/// One dropdown: its options, its choice and its one element `elem`.
pub struct Dropdown {
    /// The page whose session choice this keeps (`type_filter`'s memory); `None`: its owner keeps it.
    page: Option<Page>,
    entry: EntryId,
    elem: u32,
    kinds: Vec<String>,
    /// The options' labels when the owner names them; empty: each type's `type_filter::label`.
    names: Vec<String>,
    chosen: String,
    /// The button's word while *All* is chosen ("All types"); `None`: the option's own label.
    all: Option<fn() -> &'static str>,
    /// The button's width, measured once per value; `None` after it moves.
    width: RefCell<Option<f32>>,
}

impl Dropdown {
    /// A page's type filter, its choice kept for the session.
    pub fn new(page: Page, entry: EntryId, elem: u32) -> Self {
        Dropdown { chosen: type_filter::remembered(page), ..Self::row(entry, elem) }.page(page)
    }

    /// A dropdown whose choice its owner keeps (it hands it over with [`set_chosen`](Self::set_chosen)).
    pub fn row(entry: EntryId, elem: u32) -> Self {
        Dropdown { page: None, entry, elem, kinds: Vec::new(), names: Vec::new(), chosen: String::new(), all: None, width: Default::default() }
    }

    fn page(mut self, page: Page) -> Self {
        self.page = Some(page);
        self
    }

    /// The button's word while *All* is chosen.
    pub fn all_label(mut self, f: fn() -> &'static str) -> Self {
        self.all = Some(f);
        self
    }

    /// The options `(key, label)` now, the owner naming each; `true` when they changed.
    pub fn sync_named(&mut self, options: Vec<(String, String)>) -> bool {
        let (kinds, names): (Vec<String>, Vec<String>) = options.into_iter().unzip();
        let changed = kinds != self.kinds || names != self.names;
        if changed {
            self.kinds = kinds;
            self.names = names;
            self.width.replace(None);
        }
        changed
    }

    /// The types present now (`type_filter::chips`); `true` when they changed.
    pub fn sync(&mut self, kinds: Vec<String>) -> bool {
        let changed = kinds != self.kinds || !self.names.is_empty();
        if changed {
            self.kinds = kinds;
            self.names.clear();
            self.width.replace(None);
        }
        changed
    }

    /// Make `key` the choice without remembering it (the owner's memory restoring it).
    pub fn set_chosen(&mut self, key: &str) {
        if self.chosen != key {
            self.chosen = key.to_string();
            self.width.replace(None);
        }
    }

    /// Is the button shown at all?
    pub fn shown(&self) -> bool {
        !self.kinds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// The key chosen, `None` for *All*. A remembered key no option offers any more (its addon
    /// was removed) is *All*, without forgetting it — or, with no *All* option, the first.
    pub fn kind(&self) -> Option<&str> {
        if self.kinds.contains(&self.chosen) {
            return (self.chosen != ALL).then_some(self.chosen.as_str());
        }
        self.kinds.first().map(String::as_str).filter(|k| *k != ALL)
    }

    /// Does the page show items of `kind`?
    pub fn allows(&self, kind: &str) -> bool {
        self.kind().is_none_or(|k| k == kind)
    }

    /// Moves when what the page shows does (for the page's `revision`).
    pub fn rev(&self) -> u64 {
        let mut h = self.kinds.len() as u64;
        for b in self.kind().unwrap_or(ALL).bytes() {
            h = h.wrapping_mul(31).wrapping_add(u64::from(b));
        }
        h
    }

    /// The button's element.
    pub fn elem(&self) -> u32 {
        self.elem
    }

    /// Is `elem` this (shown) button?
    pub fn is(&self, elem: u32) -> bool {
        elem == self.elem && self.shown()
    }

    /// Apply option `n` (a pick from the popover); `true` when the page now shows something else.
    pub fn choose(&mut self, n: usize) -> bool {
        let Some(kind) = self.kinds.get(n).cloned() else { return false };
        let before = self.kind().map(str::to_string);
        self.chosen = kind;
        self.width.replace(None);
        if let Some(page) = self.page {
            type_filter::remember(page, &self.chosen);
        }
        self.kind().map(str::to_string) != before
    }

    /// The chosen option's position.
    pub fn chosen_index(&self) -> Option<usize> {
        let k = self.kind().unwrap_or(ALL);
        self.kinds.iter().position(|o| o == k)
    }

    /// Option `n`'s label.
    pub fn label(&self, n: usize) -> String {
        self.names.get(n).cloned().unwrap_or_else(|| type_filter::label(&self.kinds[n]))
    }

    /// What the button says: the chosen option (*All* in the button's own word).
    pub fn value(&self) -> String {
        match (self.kind(), self.all) {
            (None, Some(all)) => all().to_string(),
            _ => self.chosen_index().map(|n| self.label(n)).unwrap_or_default(),
        }
    }

    /// The button's width.
    pub fn width(&self, measure: &dyn Measure) -> f32 {
        *self.width.borrow_mut().get_or_insert_with(|| face_w(&self.value(), measure))
    }

    pub fn focused(&self, focus: Option<FocusKey<u32>>) -> bool {
        focus.is_some_and(|k| k.entry == self.entry && k.elem == self.elem)
    }

    /// The button at `rect`.
    pub fn draw(&self, f: &mut DrawFrame<'_, '_, Bundle>, rect: Rect) {
        draw_face(f.painter.alpha(f.page_alpha), rect, &self.value(), self.focused(f.focus.current), f.measure);
    }

    /// The popover: every option, the chosen one checked (and focused on opening).
    pub fn menu(&self, title: &str, target: ChoiceTarget) -> MenuKind {
        MenuKind::Choice { title: title.to_string(), options: (0..self.len()).map(|n| self.label(n)).collect(), current: self.chosen_index().unwrap_or(0), target }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_platform::i18n::{language_on_this_thread_for_test, Preference, SHIPPED};

    #[test]
    fn a_choice_is_kept_for_the_session_and_a_vanished_type_shows_everything() {
        let mut c = Dropdown::new(Page::Library, EntryId(1), 0x100);
        c.sync(type_filter::chips(["movie", "series"]));
        assert_eq!(c.kind(), None);
        assert!(c.choose(2), "Series");
        assert_eq!((c.kind(), c.allows("series"), c.allows("movie")), (Some("series"), true, false));
        assert!(!c.choose(2), "choosing it again changes nothing");
        assert!(!c.choose(9), "not an option");
        let mut again = Dropdown::new(Page::Library, EntryId(2), 0x100);
        again.sync(type_filter::chips(["movie", "series"]));
        assert_eq!(again.kind(), Some("series"), "a remount remembers it");
        assert_eq!(Dropdown::new(Page::Home, EntryId(3), 0x100).chosen, ALL, "per page");
        again.sync(type_filter::chips(["movie", "tv"]));
        assert_eq!(again.kind(), None, "its addon is gone: every type");
        again.sync(type_filter::chips(["movie", "series"]));
        assert_eq!(again.kind(), Some("series"), "…without forgetting it");
        type_filter::remember(Page::Library, ALL);
    }

    #[test]
    fn the_button_says_the_choice_and_the_menu_checks_it() {
        let _g = language_on_this_thread_for_test(Preference::En);
        let mut c = Dropdown::row(EntryId(1), 7).all_label(msg::app_filter_all_types);
        c.sync(type_filter::chips(["movie", "series"]));
        assert_eq!(c.value(), "All types");
        let MenuKind::Choice { options, current, .. } = c.menu("Type", ChoiceTarget::HomeType) else { panic!() };
        assert_eq!((options, current), (vec!["All".to_string(), "Movies".into(), "Series".into()], 0));
        c.choose(2);
        assert_eq!(c.value(), "Series");
        assert!(matches!(c.menu("Type", ChoiceTarget::HomeType), MenuKind::Choice { current: 2, .. }));
        assert!(c.is(7) && !c.is(8));
    }

    #[test]
    fn the_popover_hangs_under_its_button_inside_the_screen() {
        let arg = |r: Rect, target| MenuArg {
            host: EntryId(1),
            reply: sn_machine::machine::MachineId::Nav,
            focus: None,
            anchor: MenuArg::anchor_of(r),
            kind: MenuKind::Choice { title: String::new(), options: Vec::new(), current: 0, target },
        };
        let b = Rect::new(400.0, 150.0, 300.0, H);
        let p = panel(&arg(b, ChoiceTarget::DiscoverGenre), 200.0, 5000.0).unwrap();
        assert_eq!(p.x, b.x, "left edges aligned");
        assert!(p.y >= b.y + b.h, "under the button");
        assert!(p.w >= b.w, "at least the button's width");
        assert!(p.y + p.h <= SCR_H - EDGE, "a long list is clipped to the screen and scrolls");
        let right = Rect::new(SCR_W - MARGIN_X - 100.0, 150.0, 100.0, H);
        let p = panel(&arg(right, ChoiceTarget::DiscoverGenre), 400.0, 200.0).unwrap();
        assert!(p.x + p.w <= SCR_W - MARGIN_X, "pulled inside the screen");
        assert!(panel(&arg(b, ChoiceTarget::PlayerAudio), 200.0, 200.0).is_none(), "a player menu is the player's");
    }

    #[test]
    fn the_row_runs_from_the_margin_and_the_tail_stands_on_the_right() {
        let r = row(&[200.0, 300.0], Some(150.0), 10.0);
        assert_eq!(r[0].x, MARGIN_X);
        assert_eq!(r[1].x, MARGIN_X + 200.0 + GAP);
        assert_eq!(r[2].x + r[2].w, SCR_W - MARGIN_X);
        assert!(row(&[1500.0], Some(150.0), 0.0)[1].x >= MARGIN_X + 1500.0, "never over a button");
    }

    /// The app's own button words fit their button uncapped, and the multi-select's rows and
    /// title fit the popover, in every shipped language.
    #[test]
    fn the_dropdown_words_fit_in_every_shipped_language() {
        let m = sn_base::fontcov::advances::ShippedMeasure;
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let words = [
                msg::app_filter_all_types().to_string(),
                msg::app_discover_all_genres().to_string(),
                msg::app_search_addons_all().to_string(),
                msg::app_search_addons_some(12, 12),
                crate::discover::sub_tab_label(crate::discover::LIVE),
            ];
            for w in words {
                let c = CString::new(w.clone()).unwrap();
                let natural = Button::pill_w_measured(&c, SIZE, false, true, &m);
                assert!(natural <= BUTTON_W_CAP, "{}: {w:?} is {natural}px", language.tag());
            }
            for title in [msg::app_filter_type(), msg::app_discover_catalog(), msg::app_discover_genre(), msg::app_search_addons()] {
                let kind = MenuKind::List {
                    title: title.to_string(),
                    rows: [msg::app_search_select_all(), msg::app_search_select_none()]
                        .iter()
                        .map(|l| ListRow { label: l.to_string(), value: msg::app_search_addon_off().to_string(), server_value: false, opens: false, switch: None })
                        .collect(),
                    target: ChoiceTarget::SearchAddons,
                };
                let mut t = sn_ui::form::FormTable::<crate::menu::MenuRow, crate::menu::MenuRow, std::convert::Infallible>::new(0x1000);
                t.table.compact = true;
                t.set(crate::menu::build(&kind, crate::registry::SessionView::default(), crate::stores::library::AddState::Absent), None);
                out.extend(t.table.app_fit_failures_hugged(&format!("{} {title}", language.tag())));
            }
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }
}
