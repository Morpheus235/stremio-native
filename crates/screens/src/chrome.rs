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
//! Order per frame: [`ChromeBar::capture`] before the dispatcher's frame (so the frame's input
//! composes the bar this frame shows), [`ChromeBar::update`] after it (the strip's springs and the
//! members at their new scroll), [`ChromeBar::draw`] from `Rig::draw_chrome`.

use sn_machine::machine::{FocusKey, Measure};
use sn_ui::dispatch::Dispatcher;
use sn_ui::frame::glass::GlassPlan;
use sn_ui::screen::{NavPresentation, ScreenArg};
use sn_ui::widgets::{self, StripRender, TabLabels, TopFocus};

use crate::registry::{AppArg, Bundle};
use crate::tabs::{current_tab, Tab, TABS};

pub struct ChromeBar {
    labels: Vec<String>,
    keys: Vec<u32>,
    widths: Vec<f32>,
    strip: StripRender,
    /// The bar position of the selected destination.
    selected: usize,
    /// Whether the page up wears the bar this frame.
    shown: bool,
}

impl Default for ChromeBar {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeBar {
    pub fn new() -> Self {
        ChromeBar { labels: Vec::new(), keys: Vec::new(), widths: Vec::new(), strip: StripRender::new(), selected: 0, shown: false }
    }

    /// The pills' labels, in bar order. The Search pill's is empty: the strip draws it as the
    /// magnifier.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    fn vocabulary(&mut self, measure: &dyn Measure) {
        if !self.labels.is_empty() {
            return;
        }
        self.labels = TABS.iter().map(|e| if e.tab == Tab::Search { String::new() } else { e.tab.label().to_string() }).collect();
        self.keys = TABS.iter().map(|e| e.tab.elem()).collect();
        self.widths = widgets::tab_widths(&self.labels, measure);
    }

    /// What in the bar holds the remote.
    pub fn focus(&self, focus: Option<FocusKey<u32>>) -> TopFocus {
        match focus.map(|f| f.elem) {
            Some(elem) => self.keys.iter().position(|k| *k == elem).map_or(TopFocus::Away, TopFocus::Pill),
            None => TopFocus::Away,
        }
    }

    /// Publish the bar for the page being shown (or mounted this frame): its members, and the
    /// selected destination's pill as the focus fallback. A page that does not wear the bar gets
    /// none, so its focus can never land on a pill.
    pub fn capture(&mut self, d: &mut Dispatcher<Bundle>, measure: &dyn Measure) {
        self.vocabulary(measure);
        let wears = d.top_or_mounting_arg().is_none_or(|a| a.chrome() == sn_machine::machine::Chrome::TabBar);
        self.shown = wears;
        if !wears {
            d.nav.tabs.strip.clear();
            d.nav.tabs.strip_fallback = None;
            return;
        }
        self.selected = selected(d);
        d.nav.tabs.strip_fallback = Tab::at(self.selected).map(Tab::elem);
        let focus = self.focus(d.focus());
        widgets::tab_members(&self.widths, &self.keys, self.selected as i32, focus, self.strip.scroll_pos(), &mut d.nav.tabs.strip);
    }

    /// Step the strip's scroll, capsules and the tab band's material, and republish the members
    /// at the scroll they moved to.
    pub fn update(&mut self, d: &mut Dispatcher<Bundle>, glass: Option<&mut GlassPlan>, dt: f32) {
        if !self.shown || self.labels.is_empty() {
            return;
        }
        self.selected = selected(d);
        let focus = self.focus(d.focus());
        let labels = TabLabels { generation: 1, labels: &self.labels };
        self.strip.update(labels, self.selected as i32, focus, dt);
        if let Some(glass) = glass {
            glass.step_tab_band(dt);
        }
        d.nav.tabs.strip.clear();
        widgets::tab_members(&self.widths, &self.keys, self.selected as i32, focus, self.strip.scroll_pos(), &mut d.nav.tabs.strip);
    }

    /// `Rig::draw_chrome`: the bar over a page that wears it. Nothing without the application's
    /// glass plan (a host test rig), whose band is the track's material.
    pub fn draw(&mut self, arg: &AppArg, nav: NavPresentation, glass: Option<&mut GlassPlan>) {
        if arg.chrome() != sn_machine::machine::Chrome::TabBar || self.labels.is_empty() {
            return;
        }
        let Some(glass) = glass else { return };
        let p = sn_ui::Painter::root().alpha(nav.chrome_alpha);
        let labels = TabLabels { generation: 1, labels: &self.labels };
        self.strip.draw(labels, p, glass.tab_band_mut());
    }
}

/// The selected destination: the one a transition in flight is going to, else the topmost tab
/// page on the stack.
fn selected(d: &Dispatcher<Bundle>) -> usize {
    d.nav
        .tabs
        .stack
        .pending_dest()
        .and_then(Tab::of_arg)
        .or_else(|| current_tab(d).map(|(t, _)| t))
        .unwrap_or(Tab::Home)
        .index()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_machine::machine::EntryId;

    #[test]
    fn the_bar_reports_its_focus_by_destination_identity() {
        let mut bar = ChromeBar::new();
        bar.vocabulary(&sn_ui::fixture::FixtureMeasure);
        assert_eq!(bar.labels().len(), TABS.len());
        assert_eq!(bar.labels().last().map(String::as_str), Some(""), "the Search pill is the magnifier");
        let at = |tab: Tab| Some(FocusKey { entry: EntryId(1), elem: tab.elem() });
        assert_eq!(bar.focus(at(Tab::Home)), TopFocus::Pill(0));
        assert_eq!(bar.focus(at(Tab::Settings)), TopFocus::Pill(Tab::Settings.index()));
        assert_eq!(bar.focus(Some(FocusKey { entry: EntryId(1), elem: 7 })), TopFocus::Away);
    }
}
