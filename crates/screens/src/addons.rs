//! **Addons** — the addons StremioNative is using, a top-bar destination.
//!
//! Until the sign-in work lands this lists the install the app runs on (the signed-out defaults,
//! Cinemeta) as the catalog store holds it — each addon's name and version, or that it is still
//! loading, or that it failed — and says that signing in brings the user's own collection. The
//! rows are read-outs, not controls, so the remote stays on the bar. The sign-in work fills this
//! page (installing, removing, configuring); its tab and pill stay.

use std::borrow::Cow;
use std::convert::Infallible;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::form::{Form, FormSection, FormTable};
use sn_ui::route_screen::RouteLayout;
use sn_ui::screen::{DrawFrame, FocusTarget, Part, RenderStrategy, Screen, ScreenEvent};
use sn_ui::table_screen::{Header, TableScreen};
use sn_ui::{theme, Rect};

use crate::registry::{word, Bundle, FirstSeat};
use crate::stores::catalog::{CatalogStore, SourceState};
use crate::stores::StoreId;
use crate::tabs::Tab;

const GROUP: GroupId = GroupId(0);

/// The page's read-out rows, from the install as the catalog store holds it. Addon names and
/// versions are the addons' own (server text); a source that failed or is loading is named by
/// the app, never by its URL (which can carry the addon's configuration).
pub fn addons_form(store: &CatalogStore) -> Form<(), (), Infallible> {
    let mut s = FormSection::new(msg::app_addons_installed());
    for source in store.sources() {
        s = s.note(match &source.state {
            SourceState::Ready(a) => {
                let name = if a.manifest.name.is_empty() { a.manifest.id.as_str() } else { a.manifest.name.as_str() };
                if a.manifest.version.is_empty() {
                    name.to_string()
                } else {
                    format!("{name} \u{b7} {}", a.manifest.version)
                }
            }
            SourceState::Pending | SourceState::Loading => msg::app_addons_loading().to_string(),
            SourceState::Failed(e) => msg::app_addons_failed(&e.to_string()),
        });
    }
    Form::new().section(s)
}

pub struct AddonsScreen {
    entry: EntryId,
    form: FormTable<(), (), Infallible>,
    built: Option<u32>,
    seat: FirstSeat,
}

impl AddonsScreen {
    pub fn new(entry: EntryId) -> Self {
        AddonsScreen { entry, form: FormTable::new(0x10), built: None, seat: FirstSeat::new() }
    }

    pub fn table(&self) -> &sn_ui::table::TableView {
        &self.form.table
    }

    fn sync(&mut self, store: &CatalogStore) {
        if self.built != Some(store.gen()) {
            self.built = Some(store.gen());
            self.form.set(addons_form(store), None);
        }
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(RouteLayout::below_top_bar(), None, msg::app_tab_addons(), msg::app_addons_copy()),
            &self.form.table,
            GROUP,
            self.entry,
        )
        .keyed(&self.form)
    }
}

impl LogicalState for AddonsScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0).u32(self.form.table.n_rows() as u32);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("addons rows={}", self.form.table.n_rows()));
    }
}

impl Machine<Bundle> for AddonsScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.sync(cx.views.catalog);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Catalog.ord() => {
                self.sync(cx.views.catalog);
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.sync(cx.views.catalog);
                self.form.table.update(t.dt(), RouteLayout::below_top_bar().sectioned_table().h);
                let seated = crate::tabs::rests(self.entry, Tab::Addons, cx, false, false);
                self.seat.tick(seated, Some(FocusTarget::Elem(FocusKey { entry: self.entry, elem: Tab::Addons.elem() })), fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Addons, None, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry && crate::tabs::on_strip(k.elem)) {
                    crate::tabs::on_pill(k.elem, Tab::Addons, None, fx);
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) => {
                crate::tabs::on_back(self.entry, Tab::Addons, cx, fx)
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

sn_ui::focusable_via_view!(AddonsScreen, Bundle, view);

impl Screen<Bundle> for AddonsScreen {
    fn name(&self) -> &'static str {
        word::ADDONS
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        None
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
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}
