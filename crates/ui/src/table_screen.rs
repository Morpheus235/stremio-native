//! **The route family's screens as COMPONENTS** (restructure spec §10, phase 5a): `Header`
//! (crumb → title → copy, the narrative column), `TableScreen` (a header beside a `TableView`
//! in the content column — Settings' root, the Legal index) and `DocumentScreen` (a header
//! beside a `DocumentReader` — every Legal document, Privacy, About). Each is `Composed` of
//! `Part`s, so it answers the focus protocol (spec §7.1) through `composed_*` and draws through
//! `composed_draw`; and each also draws for the LEGACY loop through `paint(Painter)`, the same
//! routine, so the pixels are one function whichever loop calls it.
//!
//! What the components ADD over a bare `TableView`/`DocumentReader` is the focus protocol as data
//! (spec §10's reading of route_screen's rules): the table is a `Column` group with
//! `Seat::Remembered` (rules 1, 3, 5), its LEFT edge is `EdgeRule::Nav(Back)` (rule 9 — the crumb
//! is literal) unless the screen holds an uncommitted edit, in which case it is `Stop`; its RIGHT
//! edge is `EdgeRule::Screen`, so a chevron row's RIGHT reaches the screen's own `step` (rule 8)
//! and a plain row's is dropped; a document is a `Document` group that scrolls inside and leaves
//! at its ends. The band (rules 2, 4, 6, 7) is [`BandPart`], this module's own struct with its own
//! focus/hit logic honouring those rules, attached with [`TableScreen::with_band`] — NOT a wrapper
//! over `route_screen::ActionRow`, which has no caller left outside that module's own tests.
//!
//! **Phase 5b completed the handover this module was built for, so read the paragraph above as
//! the CURRENT contract rather than as a staging note.** 5a extracted these views while the words,
//! the statics and the `RouteFocus` ladders stayed behind in `ui/settings.rs`, `ui/legal.rs`,
//! `ui/consent.rs` and `ui/onboard.rs`; 5b moved all four onto owned screens under
//! `crate::screens` and DELETED those four modules, so there is no legacy ladder left to answer a
//! key and `band` is no longer `None` — `screens::consent` and `screens::onboard` both attach one.
//! The `paint(Painter)` entry points survive that deletion and are still exercised, but by owned
//! screens choosing to draw imperatively (`screens/onboard.rs`, `screens/consent.rs`) rather than
//! by a frame loop that owns the state; `composed_draw` is the other half and neither is dead.
//!
//! **Focus elements are row indices, except for a form-backed table.** `TablePart::keys` (a
//! `ui::form::RowKeys`, i.e. a `FormTable`) makes the element a row's `RowKey`, and all five hooks
//! plus the pointer stops translate key <-> index; without it the element is the index.

use crate::View;
use std::ffi::CStr;

use super::document_reader::DocumentReader;
use super::form::{RowKey, RowKeys};
use super::frame::Budget;
use super::geom::{Document, IndexElem, Table};
use sn_machine::machine::{Cx, EntryId, FocusKey, GroupId, Host, Measure, NavOpKind, PartId};
use super::route_screen::RouteLayout;
use super::screen::{
    composed_draw, composed_group_of, composed_groups, composed_neighbour, composed_place,
    composed_prepare, composed_reconcile, composed_seat, Activate, At, AxisMask, Composed, Dir,
    DrawFrame, EdgeRule, ElemKind, Focusable, GroupKind, GroupSpec, Hover, Part, Placed, Seat,
    Step, Stop,
};
use super::widgets::{ControlPalette, KeyHint};
use super::table::TableView;
use super::{theme, Painter, Rect};

/// The narrative column: crumb (where BACK goes, `None` = nowhere inside the app), title, copy.
/// Borrows its words for the frame; a screen's strings are constants or store-owned.
pub struct Header<'a> {
    pub layout: RouteLayout,
    pub crumb: Option<&'a str>,
    pub title: &'a str,
    pub copy: &'a str,
    pub copy_size: std::os::raw::c_int,
    /// One key-cap line stacked under the copy's measured height (the Language page's "Hold
    /// [BACK] to close StremioNative"). Never elided: a screen that sets one owns a fit test for it
    /// against [`RouteLayout`]'s narrative width.
    pub hint: Option<KeyHint<'a>>,
}

impl<'a> Header<'a> {
    pub fn new(layout: RouteLayout, crumb: Option<&'a str>, title: &'a str, copy: &'a str) -> Self {
        Self {
            layout,
            crumb,
            title,
            copy,
            copy_size: theme::size::LABEL,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: KeyHint<'a>) -> Self {
        self.hint = Some(hint);
        self
    }

    pub fn with_copy_size(mut self, sz: std::os::raw::c_int) -> Self {
        self.copy_size = sz;
        self
    }

    /// The one drawing routine, for both loops.
    pub fn paint(&self, p: Painter, measure: &dyn Measure) {
        let copy_end = self.layout.draw_narrative(p, self.crumb, self.title, self.copy, self.copy_size, measure);
        if let Some(hint) = &self.hint {
            let top = copy_end + theme::space::MD;
            // The copy already stops short of the action band; the hint keeps the same margin.
            if top + KeyHint::height() <= self.layout.action.y - theme::space::XL {
                hint.draw(p, self.layout.narrative.x, top + KeyHint::height() * 0.5, measure);
            }
        }
    }
}

impl<H: Host> Focusable<H> for Header<'_> {
    fn groups(&self, _cx: &Cx<'_, H>, _out: &mut Vec<GroupSpec>) {}
    fn group_of(&self, _key: &H::Elem, _cx: &Cx<'_, H>) -> Option<GroupId> {
        None
    }
    fn neighbour(&self, _key: FocusKey<H::Elem>, _dir: Dir, _cx: &Cx<'_, H>) -> Step<H::Elem> {
        Step::Edge
    }
    fn place(&self, _key: &H::Elem, _cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        None
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        want
    }
    fn seat(&self, _g: GroupId, from: Placed, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        // never a destination (no group); an engine that asks anyway gets the caller's own key
        FocusKey {
            entry: EntryId(0),
            elem: unreachable_elem::<H>(from),
        }
    }
}

fn unreachable_elem<H: Host>(_from: Placed) -> H::Elem
where
{
    // A `Header` contributes no group, so the engine can never seat into it; reaching this is a
    // bug in the caller, and a panic names it rather than inventing a key.
    unreachable!("Header has no focusable element")
}

impl<H: Host> Part<H> for Header<'_> {
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, _rect: Rect) {
        self.paint(f.painter, f.measure);
    }
}

/// The content column as a table: a `Column` group (rules 1, 3, 5 through `Seat::Remembered`)
/// whose LEFT edge is the crumb (rule 9) and whose RIGHT edge asks the screen (rule 8).
pub struct TablePart<'a> {
    pub table: &'a TableView,
    pub frame: Rect,
    pub group: GroupId,
    pub entry: EntryId,
    /// Rule 9's guard: `true` while the screen holds an edit BACK would discard — LEFT is a wall.
    pub uncommitted: bool,
    /// A band sits beside the table this frame: LEFT is the geometric move into it (rule 5)
    /// rather than BACK (rule 9), and DOWN off the last row finds it (rule 2).
    pub has_band: bool,
    /// A form-backed table's row-key map. Present: a row's focus element IS its [`RowKey`] number
    /// (a reorder moves no key) and every hook and pointer stop translates key <-> row index.
    /// Absent: the element is the row index, as for every table not yet on a `FormTable`.
    pub keys: Option<&'a dyn RowKeys>,
}

impl TablePart<'_> {
    /// The element the row-index view (`geom::Table`) understands for focus element `e`; `None`
    /// when the map does not know it (a stale key, the band's elements).
    fn to_index<E: IndexElem>(&self, e: E) -> Option<E> {
        match self.keys {
            None => Some(e),
            Some(k) => k.index_of_key(RowKey(e.index()?)).map(|i| E::of_index(i as u32)),
        }
    }

    /// The focus element for the row-index element `e`.
    fn to_key<E: IndexElem>(&self, e: E) -> E {
        match self.keys {
            None => e,
            Some(k) => e
                .index()
                .and_then(|i| k.key_at(i as usize))
                .map_or(e, |key| E::of_index(key.0)),
        }
    }

    fn view(&self) -> Table<'_> {
        Table {
            table: self.table,
            frame: self.frame,
            group: self.group,
            entry: self.entry,
        }
    }

    pub fn paint(&self, p: Painter, measure: &dyn Measure) {
        // a route's content column is a PAGE: its rows run off the bottom of the screen, not the
        // safe-area frame ([`TableView::draw_page`])
        self.table.draw_page(p, self.frame, measure);
    }
}

impl<H: Host> Focusable<H> for TablePart<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let n = out.len();
        Focusable::<H>::groups(&self.view(), cx, out);
        if let Some(g) = out.get_mut(n) {
            // [up, down, left, right]: up/down are the column's own (Geometric past the ends,
            // which is how DOWN off the last row reaches a band beneath — rule 2); LEFT is BACK
            // unless an edit is at stake (rule 9); RIGHT is the screen's to answer (rule 8).
            g.edge[2] = if self.has_band {
                EdgeRule::Geometric
            } else if self.uncommitted {
                EdgeRule::Stop
            } else {
                EdgeRule::Nav(NavOpKind::Back)
            };
            g.edge[3] = EdgeRule::Screen;
            // a row commits on the DOWN edge, as every row in the app does (no press dip)
            g.elem = ElemKind::Bare;
        }
    }
    fn group_of(&self, key: &H::Elem, cx: &Cx<'_, H>) -> Option<GroupId> {
        Focusable::<H>::group_of(&self.view(), &self.to_index(*key)?, cx)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        let Some(elem) = self.to_index(key.elem) else {
            return Step::Edge;
        };
        match Focusable::<H>::neighbour(&self.view(), FocusKey { entry: key.entry, elem }, dir, cx) {
            Step::Move(to) => Step::Move(FocusKey { entry: to.entry, elem: self.to_key(to.elem) }),
            Step::Edge => Step::Edge,
        }
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        // `Placed.index` stays an INDEX in its group (its readers seat by geometry), keyed or not
        Focusable::<H>::place(&self.view(), &self.to_index(*key)?, cx, at)
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        // a rebuild's identity landing wins over the engine's key; a key the map no longer knows
        // settles on the table's own selection (never row 0)
        let elem = match self.keys {
            None => want.elem,
            Some(k) => k
                .reseat()
                .and_then(|r| k.index_of_key(r))
                .or_else(|| k.index_of_key(RowKey(want.elem.index()?)))
                .or_else(|| usize::try_from(self.table.sel).ok())
                .map_or_else(|| H::Elem::of_index(0), |i| H::Elem::of_index(i as u32)),
        };
        let got = Focusable::<H>::reconcile(&self.view(), FocusKey { entry: want.entry, elem }, cx);
        FocusKey { entry: got.entry, elem: self.to_key(got.elem) }
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        let got = Focusable::<H>::seat(&self.view(), g, from, cx);
        FocusKey { entry: got.entry, elem: self.to_key(got.elem) }
    }
}

impl<H: Host> Part<H> for TablePart<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, rect: Rect) {
        let p = f.painter;
        self.frame = rect;
        self.paint(p, f.measure);
        // A walk that does not feed the hit map places no stop, and does none of the work to place
        // one (`DrawFrame::stop` would refuse each of them): the page closure runs up to four
        // times a frame, and only the visible pass counts.
        if !f.records_stops() {
            return;
        }
        // every selectable row ON THE PAGE is a stop (rule 11: hover parks, a click activates). A
        // row past the rectangle the page draws into (`TableView::page_clip`) or scrolled wholly
        // above the frame has no visible part, so no pointer can reach it; a long list (a stream
        // list's hundreds) registers the few rows it shows, from one walk of the table.
        let page = TableView::page_clip(rect);
        let (top, bottom) = (page.y, page.y + page.h);
        self.table.for_each_row_frame(rect, |i, r| {
            if r.y + r.h <= top || r.y >= bottom {
                return;
            }
            f.stop(
                p,
                Stop {
                    key: FocusKey {
                        entry: self.entry,
                        elem: self.to_key(H::Elem::of_index(i as u32)),
                    },
                    rect: r,
                    rest_rect: r,
                    clip: rect,
                    hover: Hover::Focus,
                    activate: Activate::Direct,
                },
            );
        });
    }
}

/// **The action band** (spec §10 `TableScreen{table, band}`; route_screen's rules 2, 4, 6, 7):
/// up to two pills at the bottom of the narrative column. They form a `Row` when their measured
/// labels fit, or a `Column` when they need two rows, with `Seat::Remembered`
/// whose UP above the first control leaves geometrically (rule 3), whose DOWN after the last is the floor
/// (rule 4), whose LEFT off the leading control is BACK unless an edit is at stake (rule 9), and
/// whose RIGHT off the trailing control returns to the content column geometrically (rule 7).
/// A stacked pair uses UP/DOWN between peers and RIGHT to reach the content column.
/// Every control is a `Control` element (a non-holdable press with the tvOS dip) and a hit stop.
///
/// Geometry from the `Measure` capability rather than `Button::pill_w`, so the band's placement
/// is host-testable; the DRAW measures through the same font, so the two agree on the device.
pub struct BandPart<'a> {
    pub layout: RouteLayout,
    pub labels: &'a [&'a CStr],
    pub group: GroupId,
    pub entry: EntryId,
    /// Rule 9's guard, on the band's leading edge.
    pub uncommitted: bool,
    /// The focus pop scale per control (the page's `CtlPop`), read at draw.
    pub scales: [f32; 2],
    pub palette: ControlPalette,
    /// The one control drawn in the danger face (an alert's destructive answer), if any.
    pub danger: Option<usize>,
}

/// `Button::pill_w`'s formula over a `Measure`: label width plus the pill's air.
pub fn pill_w(m: &dyn Measure, label: &CStr, sz: std::os::raw::c_int) -> f32 {
    m.width(label, sz, false) + super::widgets::BTN_PILL_AIR
}

impl BandPart<'_> {
    /// Where each control sits: one pill at the band's leading edge, or the shared pair rule.
    pub fn rects(&self, m: &dyn Measure) -> Vec<Rect> {
        let a = self.layout.action;
        match self.labels {
            [] => Vec::new(),
            [one] => {
                let w = pill_w(m, one, theme::size::BODY).min(a.w);
                vec![Rect::new(a.x, a.y, w, a.h)]
            }
            [l, t, ..] => {
                let (lr, tr) = self
                    .layout
                    .action_pair(pill_w(m, l, theme::size::BODY), pill_w(m, t, theme::size::BODY));
                vec![lr, tr]
            }
        }
    }

    /// The band's actual occupied region, including a second row when labels reflow.
    pub fn extent(&self, measure: &dyn Measure) -> Rect {
        self.rects(measure).into_iter().reduce(|a, b| a.union(b)).unwrap_or(self.layout.action)
    }

    fn stacked(&self, measure: &dyn Measure) -> bool {
        let rects = self.rects(measure);
        rects.len() > 1 && rects[0].y != rects[1].y
    }

    fn key<H: Host>(&self, i: usize) -> FocusKey<H::Elem>
    where
        H::Elem: IndexElem,
    {
        FocusKey {
            entry: self.entry,
            elem: H::Elem::of_index(BAND_BASE + i as u32),
        }
    }
}

/// The band's controls sit above this in a screen's element namespace (`screens::registry::BAND`
/// is the same number; the library cannot name the registry).
pub const BAND_BASE: u32 = 0x4000_0000;

impl<H: Host> Focusable<H> for BandPart<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        if self.labels.is_empty() {
            return;
        }
        out.push(GroupSpec {
            id: self.group,
            kind: if self.stacked(cx.measure) { GroupKind::Column } else { GroupKind::Row { wrap: false } },
            seat: Seat::Remembered,
            reachable: AxisMask::BOTH,
            edge: [
                EdgeRule::Geometric,
                EdgeRule::Stop,
                if self.uncommitted { EdgeRule::Stop } else { EdgeRule::Nav(NavOpKind::Back) },
                EdgeRule::Geometric,
            ],
            extent: self.extent(cx.measure),
            len: self.labels.len(),
            elem: ElemKind::Control,
        });
    }
    fn group_of(&self, key: &H::Elem, _cx: &Cx<'_, H>) -> Option<GroupId> {
        let i = key.index()?.checked_sub(BAND_BASE)? as usize;
        (i < self.labels.len()).then_some(self.group)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        let Some(i) = key.elem.index().and_then(|i| i.checked_sub(BAND_BASE)) else {
            return Step::Edge;
        };
        let i = i as usize;
        let dir = if self.stacked(cx.measure) {
            match dir {
                Dir::Up => Dir::Left,
                Dir::Down => Dir::Right,
                Dir::Left | Dir::Right => return Step::Edge,
            }
        } else { dir };
        match dir {
            Dir::Left if i > 0 => Step::Move(self.key::<H>(i - 1)),
            Dir::Right if i + 1 < self.labels.len() => Step::Move(self.key::<H>(i + 1)),
            _ => Step::Edge,
        }
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, _at: At) -> Option<Placed> {
        let i = key.index()?.checked_sub(BAND_BASE)? as usize;
        let r = *self.rects(cx.measure).get(i)?;
        Some(Placed {
            rect: r,
            rest_rect: r,
            clip: Rect::FULL,
            index: Some(i as u32),
        })
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        let i = want.elem.index().and_then(|i| i.checked_sub(BAND_BASE)).unwrap_or(0) as usize;
        self.key::<H>(i.min(self.labels.len().saturating_sub(1)))
    }
    fn seat(&self, _g: GroupId, _from: Placed, _cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        self.key::<H>(0)
    }
}

impl<H: Host> Part<H> for BandPart<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, _rect: Rect) {
        let p = f.painter;
        let focused = f.focus.current.and_then(|k| k.elem.index()).and_then(|e| e.checked_sub(BAND_BASE));
        let rects = self.rects(f.measure);
        let env = super::Env::inert();
        for (i, (label, r)) in self.labels.iter().zip(rects.iter()).enumerate() {
            let mut b = super::widgets::Button::new(label.as_ptr(), theme::size::BODY, *r)
                .focused(focused == Some(i as u32))
                .scale(self.scales.get(i).copied().unwrap_or(1.0))
                .palette(self.palette);
            if self.danger == Some(i) {
                b = b.style(super::widgets::ControlStyle::Danger);
            }
            b.draw(&env, p);
            f.stop(
                p,
                Stop {
                    key: self.key::<H>(i),
                    rect: *r,
                    rest_rect: *r,
                    clip: Rect::FULL,
                    hover: Hover::Focus,
                    activate: Activate::Press,
                },
            );
        }
    }
}

/// A header beside a table — the Settings root and the Legal index (spec §10 `TableScreen`).
pub struct TableScreen<'a> {
    pub header: Header<'a>,
    pub table: TablePart<'a>,
    /// The action band (rules 2, 4, 6, 7), `None` for a screen whose every row is a door.
    pub band: Option<BandPart<'a>>,
}

impl<'a> TableScreen<'a> {
    /// Over a route's own layout: the table in the content column's SECTIONED frame.
    pub fn new(header: Header<'a>, table: &'a TableView, group: GroupId, entry: EntryId) -> Self {
        let frame = header.layout.sectioned_table();
        Self {
            header,
            table: TablePart {
                table,
                frame,
                group,
                entry,
                uncommitted: false,
                has_band: false,
                keys: None,
            },
            band: None,
        }
    }

    /// The table is form-backed: its focus elements are the map's [`RowKey`]s (see
    /// [`TablePart::keys`]).
    pub fn keyed(mut self, keys: &'a dyn RowKeys) -> Self {
        self.table.keys = Some(keys);
        self
    }

    /// The table in a frame of the screen's own (first run's `content`, not the sectioned inset).
    pub fn with_frame(mut self, frame: Rect) -> Self {
        self.table.frame = frame;
        self
    }

    pub fn uncommitted(mut self, v: bool) -> Self {
        self.table.uncommitted = v;
        if let Some(b) = self.band.as_mut() {
            b.uncommitted = v;
        }
        self
    }

    /// Put a band beside the table: the table's LEFT becomes the move into it (rule 5) and the
    /// band inherits the screen's `uncommitted` guard.
    pub fn with_band(mut self, band: BandPart<'a>) -> Self {
        let uncommitted = self.table.uncommitted;
        self.table.has_band = !band.labels.is_empty();
        self.band = Some(BandPart { uncommitted, ..band });
        self
    }

    /// The legacy loop's draw: header, then table.
    pub fn paint(&self, p: Painter, measure: &dyn Measure) {
        self.header.paint(p, measure);
        self.table.paint(p, measure);
    }
}

impl<H: Host> Composed<H> for TableScreen<'_>
where
    H::Elem: IndexElem,
{
    fn layout(&self, cx: &Cx<'_, H>) -> Vec<(PartId, Rect)> {
        let mut v = vec![(PartId(0), self.header.layout.narrative), (PartId(1), self.table.frame)];
        if let Some(band) = &self.band {
            v.push((PartId(2), band.extent(cx.measure)));
        }
        v
    }
    fn part(&self, id: PartId) -> &dyn Part<H> {
        match (id, self.band.as_ref()) {
            (PartId(0), _) => &self.header,
            (PartId(2), Some(b)) => b,
            _ => &self.table,
        }
    }
    fn part_mut(&mut self, id: PartId) -> &mut dyn Part<H> {
        match (id, self.band.as_mut()) {
            (PartId(0), _) => &mut self.header,
            (PartId(2), Some(b)) => b,
            _ => &mut self.table,
        }
    }
}

impl<H: Host> Focusable<H> for TableScreen<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        composed_groups(self, cx, out)
    }
    fn group_of(&self, key: &H::Elem, cx: &Cx<'_, H>) -> Option<GroupId> {
        composed_group_of(self, key, cx)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        composed_neighbour(self, key, dir, cx)
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        composed_place(self, key, cx, at)
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        composed_reconcile(self, want, cx)
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        composed_seat(self, g, from, cx)
    }
}

impl<H: Host> Part<H> for TableScreen<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, b: &mut Budget, cx: &Cx<'_, H>) {
        composed_prepare(self, b, cx)
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, _rect: Rect) {
        composed_draw(self, f)
    }
}

/// The content column as a document: ONE element that scrolls inside and leaves at its ends.
pub struct DocumentPart<'a> {
    pub reader: &'a mut DocumentReader,
    pub frame: Rect,
    pub body: &'a str,
    pub group: GroupId,
    pub entry: EntryId,
}

/// The document's FOCUS half over an immutable reader — what a page whose `Focusable` methods
/// take `&self` builds per query, while its draw builds the `DocumentPart` over `&mut`.
pub struct DocumentFocus<'a> {
    pub reader: &'a DocumentReader,
    pub frame: Rect,
    pub group: GroupId,
    pub entry: EntryId,
}

impl DocumentFocus<'_> {
    fn view(&self) -> Document<'_> {
        Document {
            reader: self.reader,
            frame: self.frame,
            group: self.group,
            entry: self.entry,
        }
    }
}

impl<H: Host> Focusable<H> for DocumentFocus<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        let n = out.len();
        Focusable::<H>::groups(&self.view(), cx, out);
        if let Some(g) = out.get_mut(n) {
            // a document is a READ: LEFT always walks back out (rule 9 with nothing to lose)
            g.edge[2] = EdgeRule::Nav(NavOpKind::Back);
        }
    }
    fn group_of(&self, key: &H::Elem, cx: &Cx<'_, H>) -> Option<GroupId> {
        Focusable::<H>::group_of(&self.view(), key, cx)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        Focusable::<H>::neighbour(&self.view(), key, dir, cx)
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        Focusable::<H>::place(&self.view(), key, cx, at)
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        Focusable::<H>::reconcile(&self.view(), want, cx)
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        Focusable::<H>::seat(&self.view(), g, from, cx)
    }
}

impl DocumentPart<'_> {
    fn focus(&self) -> DocumentFocus<'_> {
        DocumentFocus {
            reader: self.reader,
            frame: self.frame,
            group: self.group,
            entry: self.entry,
        }
    }

    pub fn paint(&mut self, p: Painter) {
        self.reader.draw(p, self.frame, None, self.body);
    }
}

impl<H: Host> Focusable<H> for DocumentPart<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        Focusable::<H>::groups(&self.focus(), cx, out)
    }
    fn group_of(&self, key: &H::Elem, cx: &Cx<'_, H>) -> Option<GroupId> {
        Focusable::<H>::group_of(&self.focus(), key, cx)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        Focusable::<H>::neighbour(&self.focus(), key, dir, cx)
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        Focusable::<H>::place(&self.focus(), key, cx, at)
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        Focusable::<H>::reconcile(&self.focus(), want, cx)
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        Focusable::<H>::seat(&self.focus(), g, from, cx)
    }
}

impl<H: Host> Part<H> for DocumentPart<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, _b: &mut Budget, _cx: &Cx<'_, H>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, rect: Rect) {
        let p = f.painter;
        self.frame = rect;
        self.paint(p);
        f.stop(
            p,
            Stop {
                key: FocusKey {
                    entry: self.entry,
                    elem: H::Elem::of_index(0),
                },
                rect,
                rest_rect: rect,
                clip: rect,
                hover: Hover::Ignore,
                activate: Activate::Direct,
            },
        );
    }
}

/// A header beside a document — every Legal document, Privacy, About (spec §10
/// `DocumentScreen`).
pub struct DocumentScreen<'a> {
    pub header: Header<'a>,
    pub doc: DocumentPart<'a>,
}

impl<'a> DocumentScreen<'a> {
    /// Over a route's own layout: the document hangs from the TITLE's anchor (`RouteLayout::document`).
    pub fn new(header: Header<'a>, reader: &'a mut DocumentReader, body: &'a str, group: GroupId, entry: EntryId) -> Self {
        let frame = header.layout.document(header.crumb.is_some());
        Self {
            header,
            doc: DocumentPart {
                reader,
                frame,
                body,
                group,
                entry,
            },
        }
    }

    pub fn paint(&mut self, p: Painter, measure: &dyn Measure) {
        self.header.paint(p, measure);
        self.doc.paint(p);
    }
}

impl<H: Host> Composed<H> for DocumentScreen<'_>
where
    H::Elem: IndexElem,
{
    fn layout(&self, _cx: &Cx<'_, H>) -> Vec<(PartId, Rect)> {
        vec![(PartId(0), self.header.layout.narrative), (PartId(1), self.doc.frame)]
    }
    fn part(&self, id: PartId) -> &dyn Part<H> {
        match id {
            PartId(0) => &self.header,
            _ => &self.doc,
        }
    }
    fn part_mut(&mut self, id: PartId) -> &mut dyn Part<H> {
        match id {
            PartId(0) => &mut self.header,
            _ => &mut self.doc,
        }
    }
}

impl<H: Host> Focusable<H> for DocumentScreen<'_>
where
    H::Elem: IndexElem,
{
    fn groups(&self, cx: &Cx<'_, H>, out: &mut Vec<GroupSpec>) {
        composed_groups(self, cx, out)
    }
    fn group_of(&self, key: &H::Elem, cx: &Cx<'_, H>) -> Option<GroupId> {
        composed_group_of(self, key, cx)
    }
    fn neighbour(&self, key: FocusKey<H::Elem>, dir: Dir, cx: &Cx<'_, H>) -> Step<H::Elem> {
        composed_neighbour(self, key, dir, cx)
    }
    fn place(&self, key: &H::Elem, cx: &Cx<'_, H>, at: At) -> Option<Placed> {
        composed_place(self, key, cx, at)
    }
    fn reconcile(&self, want: FocusKey<H::Elem>, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        composed_reconcile(self, want, cx)
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, H>) -> FocusKey<H::Elem> {
        composed_seat(self, g, from, cx)
    }
}

impl<H: Host> Part<H> for DocumentScreen<'_>
where
    H::Elem: IndexElem,
{
    fn prepare(&mut self, b: &mut Budget, cx: &Cx<'_, H>) {
        composed_prepare(self, b, cx)
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, H>, _rect: Rect) {
        composed_draw(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{FixtureHost, FixtureMeasure, FixtureView, FixtureViews};
    use sn_machine::machine::{FocusRead, InputOwner, PressRead, Tick};
    use crate::screen::{GroupKind, Seat};
    use crate::table::{Row, Section};

    // The PARTS are asked directly rather than through `Composed` (`composed_groups` and its
    // siblings): `part()` materialises a `&dyn Part` vtable, whose `draw` reaches `TextView` and
    // SDL2_ttf — which the host suite cannot LINK (`ui/CLAUDE.md`'s boundary). The composed fns
    // are the fixture screen's subject already.
    fn cx<'a>(m: &'a dyn Measure, v: &'a FixtureView) -> Cx<'a, FixtureHost> {
        Cx {
            views: FixtureViews { store: v },
            tick: Tick::default(),
            measure: m,
            press: PressRead::default(),
            focus: FocusRead::default(),
            owner: InputOwner::Entry(EntryId(0)),
        }
    }

    const E: EntryId = EntryId(5);
    const G: GroupId = GroupId(9);

    fn same(a: Rect, b: Rect) -> bool {
        (a.x, a.y, a.w, a.h) == (b.x, b.y, b.w, b.h)
    }

    fn table(rows: usize) -> TableView {
        let mut t = TableView::new();
        let mut s = Section::new("Section");
        for i in 0..rows {
            s = s.row(Row::new(format!("row {i}")).chevron(i == 0));
        }
        t.set_sections(vec![s], 0, false);
        t
    }

    /// **A form-backed table's focus elements are its `RowKey`s, and every hook translates.** Keys
    /// deliberately unrelated to position (30, 10, 20 down the page): the same answers as the
    /// index-addressed table over the same rows, spelled in keys — including the pointer path's
    /// `place` and a key the map no longer holds.
    #[test]
    fn a_keyed_table_part_speaks_row_keys_through_all_five_hooks() {
        use crate::form::{Form, FormId, FormSection, FormTable, RowKind};
        #[derive(Clone, PartialEq)]
        struct Id(u32);
        impl FormId for Id {
            fn key(&self) -> RowKey {
                RowKey(self.0)
            }
        }
        let mut form = FormTable::<Id, (), ()>::new(100);
        let mut sec = FormSection::new("Section");
        for k in [30, 10, 20] {
            sec = sec.item(Id(k), RowKind::Button, (), Row::new(format!("row {k}")));
        }
        form.set(Form::new().section(sec), None);
        let (m, v) = (FixtureMeasure, FixtureView::default());
        let cx = cx(&m, &v);
        let frame = RouteLayout::screen().sectioned_table();
        let keyed = TablePart {
            table: &form.table, frame, group: G, entry: E,
            uncommitted: false, has_band: false, keys: Some(&form),
        };
        let plain = TablePart {
            table: &form.table, frame, group: G, entry: E,
            uncommitted: false, has_band: false, keys: None,
        };
        let fk = |elem: u32| FocusKey { entry: E, elem };
        type H = FixtureHost;

        assert_eq!(Focusable::<H>::group_of(&keyed, &30, &cx), Some(G));
        assert_eq!(Focusable::<H>::group_of(&keyed, &0, &cx), None, "an index is not a key here");
        assert_eq!(Focusable::<H>::group_of(&keyed, &99, &cx), None);

        assert!(matches!(Focusable::<H>::neighbour(&keyed, fk(30), Dir::Down, &cx),
            Step::Move(to) if to.elem == 10 && to.entry == E));
        assert!(matches!(Focusable::<H>::neighbour(&keyed, fk(20), Dir::Up, &cx),
            Step::Move(to) if to.elem == 10));
        assert!(matches!(Focusable::<H>::neighbour(&keyed, fk(30), Dir::Up, &cx), Step::Edge));
        assert!(matches!(Focusable::<H>::neighbour(&keyed, fk(20), Dir::Down, &cx), Step::Edge));

        let placed = Focusable::<H>::place(&keyed, &10, &cx, At::Drawn).unwrap();
        let by_index = Focusable::<H>::place(&plain, &1, &cx, At::Drawn).unwrap();
        assert!(same(placed.rect, by_index.rect), "key 10 is the row the index view calls 1");
        assert_eq!(placed.index, Some(1), "Placed.index stays an index in its group");
        assert!(Focusable::<H>::place(&keyed, &1, &cx, At::Drawn).is_none());

        assert_eq!(Focusable::<H>::reconcile(&keyed, fk(20), &cx).elem, 20);
        assert_eq!(Focusable::<H>::reconcile(&keyed, fk(999), &cx).elem, 30,
            "a key that left settles on the first row");

        let from = Focusable::<H>::place(&plain, &2, &cx, At::Drawn).unwrap();
        assert_eq!(Focusable::<H>::seat(&keyed, G, from, &cx).elem, 20, "nearest the row it left from");
        assert_eq!(Focusable::<H>::seat(&plain, G, from, &cx).elem, 2, "the plain table still speaks indices");
    }

    /// Spec §10: the table screen's focus protocol AS DATA — one Column group, Seat::Remembered,
    /// LEFT = BACK (rule 9) and RIGHT = the screen's (rule 8); with an uncommitted edit LEFT is a
    /// wall; the header contributes no group.
    #[test]
    fn a_table_screen_is_one_remembered_column_whose_left_edge_is_back() {
        let (m, v) = (FixtureMeasure, FixtureView::default());
        let cx = cx(&m, &v);
        let mut t = table(3);
        let layout = RouteLayout::screen();
        let ts = TableScreen::new(Header::new(layout, Some("Settings"), "Legal notices", "copy"), &mut t, G, E);
        let mut groups = Vec::new();
        Focusable::<FixtureHost>::groups(&ts.header, &cx, &mut groups);
        assert!(groups.is_empty(), "the header is not a group");
        Focusable::<FixtureHost>::groups(&ts.table, &cx, &mut groups);
        assert_eq!(groups.len(), 1);
        let g = groups[0];
        assert_eq!(g.id, G);
        assert!(matches!(g.kind, GroupKind::Column));
        assert_eq!(g.seat, Seat::Remembered);
        assert_eq!(g.len, 3);
        assert_eq!(g.edge[2], EdgeRule::Nav(NavOpKind::Back), "LEFT follows the crumb");
        assert_eq!(g.edge[3], EdgeRule::Screen, "RIGHT on a chevron row is the screen's");
        assert!(same(g.extent, layout.sectioned_table()));
        let mut t2 = table(3);
        let dirty = TableScreen::new(Header::new(layout, None, "Privacy", "copy"), &mut t2, G, E).uncommitted(true);
        let mut groups = Vec::new();
        Focusable::<FixtureHost>::groups(&dirty.table, &cx, &mut groups);
        assert_eq!(groups[0].edge[2], EdgeRule::Stop, "an edit at stake makes LEFT a wall");
    }

    /// Spec §7.1: geometry IS `place` — a row's placement is the rect `TableView` draws it at,
    /// and DOWN walks the rows to an edge.
    #[test]
    fn a_table_screen_places_rows_where_the_table_draws_them() {
        let (m, v) = (FixtureMeasure, FixtureView::default());
        let cx = cx(&m, &v);
        let mut t = table(3);
        let layout = RouteLayout::screen();
        let frame = layout.sectioned_table();
        let ts = TableScreen::new(Header::new(layout, None, "Settings", "copy"), &mut t, G, E);
        for i in 0..3u32 {
            let placed = Focusable::<FixtureHost>::place(&ts.table, &i, &cx, At::SpringTarget).expect("placed");
            assert!(same(placed.rect, ts.table.table.row_frame(frame, i as i32).unwrap()));
            assert_eq!(placed.index, Some(i));
        }
        assert!(Focusable::<FixtureHost>::place(&ts.table, &3, &cx, At::SpringTarget).is_none());
        let k = |i: u32| FocusKey { entry: E, elem: i };
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&ts.table, k(0), Dir::Down, &cx), Step::Move(m) if m.elem == 1));
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&ts.table, k(2), Dir::Down, &cx), Step::Edge));
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&ts.table, k(1), Dir::Left, &cx), Step::Edge), "LEFT is an edge: the rule is on the group");
    }

    /// Spec §10: a document screen is one `Document` group that scrolls inside and leaves at its
    /// ends, its LEFT edge BACK, hanging from the title anchor.
    #[test]
    fn a_document_screen_is_one_document_group_that_leaves_at_its_ends() {
        let (m, v) = (FixtureMeasure, FixtureView::default());
        let cx = cx(&m, &v);
        let mut r = DocumentReader::new();
        r.set_extent_for_test(400.0);
        let layout = RouteLayout::screen();
        let ds = DocumentScreen::new(Header::new(layout, Some("Legal notices"), "Privacy", "copy"), &mut r, "body", G, E);
        let mut groups = Vec::new();
        Focusable::<FixtureHost>::groups(&ds.doc, &cx, &mut groups);
        assert_eq!(groups.len(), 1);
        assert!(matches!(groups[0].kind, GroupKind::Document));
        assert_eq!(groups[0].edge[2], EdgeRule::Nav(NavOpKind::Back));
        assert!(same(groups[0].extent, layout.document(true)));
        let k = FocusKey { entry: E, elem: 0u32 };
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&ds.doc, k, Dir::Up, &cx), Step::Edge), "at the top, UP leaves");
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&ds.doc, k, Dir::Down, &cx), Step::Move(_)), "not at the end: DOWN scrolls inside");
        assert!(same(Focusable::<FixtureHost>::place(&ds.doc, &0, &cx, At::Drawn).unwrap().rect, layout.document(true)));
    }
    /// The real Belarusian consent labels overflowed the horizontal band's width and aborted
    /// the simulator. Use Unicode-scalar measurement so UTF-8 byte length cannot create this
    /// regression artificially; each individual label still fits the normal narrative width.
    #[test]
    fn long_cyrillic_actions_reflow_without_clipping_or_losing_remote_navigation() {
        struct CyrillicMeasure;
        impl Measure for CyrillicMeasure {
            fn width(&self, text: &CStr, size: i32, _bold: bool) -> f32 {
                text.to_string_lossy().chars().count() as f32 * size as f32 * 0.65
            }
            fn cap_h(&self, size: i32) -> f32 { size as f32 * 0.7 }
            fn line_h(&self, size: i32) -> f32 { size as f32 * 1.2 }
        }
        // Preserve the generic long-action fallback independently of consent's concise verbs.
        let labels = [c"Адпраўляць справаздачы", c"Не адпраўляць"];
        let layout = RouteLayout::screen();
        let band = BandPart { layout, labels: &labels, group: G, entry: E,
            uncommitted: false, scales: [1.0; 2], palette: ControlPalette::default(), danger: None };
        let (measure, view) = (CyrillicMeasure, FixtureView::default());
        let cx = cx(&measure, &view);
        let rects = band.rects(&measure);
        assert!(rects[0].y + rects[0].h <= rects[1].y);
        for (label, rect) in labels.iter().zip(&rects) {
            assert!(rect.w >= pill_w(&measure, label, theme::size::BODY));
            assert!(rect.x + rect.w <= layout.action.x + layout.action.w);
        }
        let mut groups = Vec::new();
        Focusable::<FixtureHost>::groups(&band, &cx, &mut groups);
        assert!(matches!(groups[0].kind, GroupKind::Column));
        let first = band.key::<FixtureHost>(0);
        let last = band.key::<FixtureHost>(1);
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&band, first, Dir::Down, &cx), Step::Move(k) if k == last));
        assert!(matches!(Focusable::<FixtureHost>::neighbour(&band, last, Dir::Up, &cx), Step::Move(k) if k == first));
        for (i, key) in [first, last].into_iter().enumerate() {
            let placed = Focusable::<FixtureHost>::place(&band, &key.elem, &cx, At::SpringTarget).unwrap();
            assert!(same(placed.rest_rect, rects[i]));
        }
    }

    // ---- the page's stops: only what the page shows, one walk, the same stops as the full loop ----

    /// A stream list's shape: `n` chip rows over five addon sections (the Detail page).
    fn stream_list(n: usize) -> TableView {
        use crate::table::Chip;
        let mut t = TableView::new();
        let mut secs = Vec::new();
        for a in 0..5 {
            let mut sec = Section::new(format!("Addon {a}"));
            for i in 0..n / 5 {
                let mut r = Row::new(format!("Some.Release.Name.{a}.{i}.2160p.WEB-DL.DDP5.1.Atmos.DV.HDR10.HEVC-GROUP")).side("Addon", "4K");
                for c in 0..4 {
                    r = r.chip(Chip::new(format!("Chip {c}")).server());
                }
                sec = sec.row(r);
            }
            secs.push(sec);
        }
        t.set_sections(secs, 0, false);
        t
    }

    /// Every row kind the table draws, over three sections (the second headerless, so a bare
    /// divider leads it) and enough of them to run well past the frame.
    fn mixed_table(grouped: bool) -> TableView {
        use crate::table::Chip;
        let chip = |i: usize| {
            let mut r = Row::new(format!("Some.Release.Name.{i}.2160p.WEB-DL.DDP5.1.Atmos.DV.HDR10.HEVC-GROUP another run of words")).side("Addon", "4K");
            for c in 0..8 {
                r = r.chip(Chip::new(format!("Chip number {c}")).server());
            }
            r
        };
        let mut first = Section::new("Addon").row(Row::note("A note about the rows under it")).row(Row::new("plain")).row(Row::new("two lines").detail("a sub-line"));
        first = first.row(Row::separator()).row(Row::new("after the rule").chevron(true));
        for i in 0..6 {
            first = first.row(chip(i));
        }
        let mut second = Section::new("");
        for i in 0..12 {
            second = second.row(if i % 4 == 0 { Row::new(format!("episode {i}")).detail("S1, E1").still("").blurb("an overview") } else { chip(100 + i) });
        }
        let mut third = Section::new("Closing");
        for i in 0..10 {
            third = third.row(Row::new(format!("tail {i}")));
        }
        let mut t = TableView::new();
        t.grouped = grouped;
        t.set_sections(vec![first, second, third], 7, false);
        t
    }

    /// Settle the table's scroll on its selection, as the page's ticks do.
    fn settled(mut t: TableView, frame: Rect) -> TableView {
        for _ in 0..400 {
            t.update(1.0 / 60.0, frame.h);
        }
        t
    }

    fn stops_of(t: &TableView, frame: Rect, hit_map: bool, old: bool) -> Vec<Stop<u32>> {
        let (m, v) = (FixtureMeasure, FixtureView::default());
        let cx = cx(&m, &v);
        let mut part = TablePart { table: t, frame, group: G, entry: E, uncommitted: false, has_band: false, keys: None };
        let mut run = || {
            let mut f = DrawFrame::new(&cx, Painter::recording());
            if old {
                // the loop `TablePart::draw` ran for EVERY row before it culled to the page
                let p = f.painter;
                part.paint(p, f.measure);
                for i in 0..t.n_rows() {
                    if t.next_selectable(i, 0) != Some(i) {
                        continue;
                    }
                    if let Some(r) = t.row_frame(frame, i) {
                        f.stop(p, Stop {
                            key: FocusKey { entry: E, elem: i as u32 },
                            rect: r,
                            rest_rect: r,
                            clip: frame,
                            hover: Hover::Focus,
                            activate: Activate::Direct,
                        });
                    }
                }
            } else {
                Part::<FixtureHost>::draw(&mut part, &mut f, frame);
            }
            f.into_stops()
        };
        if hit_map { crate::screen::record_stops_while_recording(run) } else { run() }
    }

    /// The bottom of what a page draws: the screen's edge, never above the frame's own.
    fn page_bottom(frame: Rect) -> f32 {
        frame.y + (crate::consts::SCR_H - frame.y).max(frame.h)
    }

    fn on_page(s: &Stop<u32>, frame: Rect) -> bool {
        s.rect.y + s.rect.h > frame.y && s.rect.y < page_bottom(frame)
    }

    fn hittable(s: &Stop<u32>) -> bool {
        let seen = s.rect.intersect(s.clip);
        seen.w > 0.0 && seen.h > 0.0
    }

    /// **One draw of a 500-row stream list registers stops for the rows on the page and walks the
    /// table a handful of times, not once per row.** Each stop used to cost a full walk to find its
    /// row (`row_frame`), so a frame was rows² row heights: 6 fps on the television at 497 streams.
    /// Counted, not timed: a clock is not deterministic, a count is.
    #[test]
    fn a_page_registers_stops_only_for_the_rows_it_shows_and_walks_the_table_once() {
        let _serial = sn_base::testlock::serial();
        let n = 500;
        let t = stream_list(n);
        let frame = RouteLayout::screen().sectioned_table();
        let _ = stops_of(&t, frame, true, false); // the first draw fits the rows
        crate::table::census::reset();
        let stops = stops_of(&t, frame, true, false);
        let (walks, row_hs, rows_ats) = crate::table::census::counts();
        assert!(!stops.is_empty() && stops.len() < 40, "{} stops for a page that shows a few rows", stops.len());
        assert!(stops.iter().all(|s| on_page(s, frame)), "a stop for a row off the page");
        assert!(walks <= 4, "{walks} walks of the table for one draw");
        assert!(row_hs <= 4 * n as u32, "{row_hs} row heights for {n} rows");
        assert!(rows_ats <= 16, "{rows_ats} linear row lookups");
    }

    /// A walk that does not feed the hit map (the text prewarm, backdrop discovery, each blur
    /// source) draws the table and does nothing else: no stop, and none of the work to place one.
    #[test]
    fn a_walk_that_feeds_no_hit_map_places_no_stop_and_walks_the_table_once() {
        let _serial = sn_base::testlock::serial();
        let t = stream_list(500);
        let frame = RouteLayout::screen().sectioned_table();
        let _ = stops_of(&t, frame, false, false);
        crate::table::census::reset();
        let stops = stops_of(&t, frame, false, false);
        let (walks, row_hs, _) = crate::table::census::counts();
        assert!(stops.is_empty());
        assert!(walks <= 2, "{walks} walks for a draw that places nothing");
        assert!(row_hs <= 2 * 500, "{row_hs} row heights");
    }

    /// **Culling changes no stop that can be hit.** Every stop the page registers is, field for
    /// field and in order, a stop the old every-row loop registered; every stop the old loop
    /// registered that the page drops is one nobody could hit (its visible part, `rect ∩ clip`, is
    /// empty); and a page hands the hit map the same answer at every point. Over every row kind
    /// (plain, note, chip, still, the focused EXPANDED chip row, headers, dividers, a rule), a
    /// grouped table, a scrolled table, and a frame short of the screen's bottom (the page runs
    /// on) and one that already reaches past it.
    #[test]
    fn the_stops_a_page_registers_are_the_old_loops_stops_that_are_on_it() {
        let _serial = sn_base::testlock::serial();
        let route = RouteLayout::screen().sectioned_table();
        let short = Rect::new(route.x, 160.0, route.w, 420.0);
        let tall = Rect::new(route.x, 300.0, route.w, 900.0);
        let mut cases = 0;
        for grouped in [false, true] {
            for (name, frame) in [("route", route), ("short", short), ("tall", tall)] {
                for sel in [0, 7, 14, 21, 27, 30] {
                    for scrolled in [false, true] {
                        let mut t = mixed_table(grouped);
                        t.sel = t.settle(sel);
                        let t = if scrolled { settled(t, frame) } else { t };
                        let what = format!("{name} frame, grouped {grouped}, sel {sel}, scrolled {scrolled}");
                        let old = stops_of(&t, frame, true, true);
                        let new = stops_of(&t, frame, true, false);
                        let want: Vec<String> = old.iter().filter(|s| on_page(s, frame)).map(|s| format!("{s:?}")).collect();
                        let got: Vec<String> = new.iter().map(|s| format!("{s:?}")).collect();
                        assert_eq!(got, want, "{what}");
                        assert!(old.iter().filter(|s| !on_page(s, frame)).all(|s| !hittable(s)), "{what}: a hittable stop was dropped");
                        let (mut was, mut now) = (crate::hit::HitMap::<u32>::new(), crate::hit::HitMap::<u32>::new());
                        was.fill(old);
                        was.swap();
                        now.fill(new);
                        now.swap();
                        let mut y = frame.y - 20.0;
                        while y < crate::consts::SCR_H + 20.0 {
                            for x in [frame.x + 3.0, frame.x + frame.w * 0.5] {
                                let key = |h: &crate::hit::HitMap<u32>| h.top_at(x, y).map(|s| s.key.elem);
                                assert_eq!(key(&now), key(&was), "{what}: point ({x}, {y})");
                            }
                            y += 7.0;
                        }
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 72);
    }

    /// A row that straddles the bottom of the frame is partly visible, so it keeps its stop: a click
    /// on the part inside the frame reaches it; the part below the frame, still drawn on a page but
    /// outside the stop's clip, answers nothing, as it always did.
    #[test]
    fn a_row_cut_by_the_frames_bottom_edge_is_still_clickable_on_its_visible_part() {
        let _serial = sn_base::testlock::serial();
        let frame = Rect::new(0.0, 100.0, 884.0, 500.0);
        let t = mixed_table(false);
        let stops = stops_of(&t, frame, true, false);
        let edge = frame.y + frame.h;
        let cut = stops.iter().find(|s| s.rect.y < edge && s.rect.y + s.rect.h > edge).expect("a row straddles the frame's bottom");
        let mut map = crate::hit::HitMap::<u32>::new();
        map.fill(stops.clone());
        map.swap();
        let x = cut.rect.x + 40.0;
        let inside = map.top_at(x, (cut.rect.y + edge) * 0.5).expect("the visible part is a target");
        assert_eq!(inside.key.elem, cut.key.elem);
        assert!(map.top_at(x, edge + 4.0).is_none(), "below the frame nothing answers");
    }

    /// Focus walks DOWN past what the page shows: the table scrolls to keep the selection in view
    /// and the row that arrives has its stop, where `row_frame` puts it, from the frame it lands in.
    #[test]
    fn focus_moving_down_past_the_page_scrolls_and_the_new_row_gets_its_stop() {
        let _serial = sn_base::testlock::serial();
        let frame = RouteLayout::screen().sectioned_table();
        let mut t = stream_list(500);
        let first = stops_of(&t, frame, true, false);
        let last_shown = first.iter().filter(|s| hittable(s)).map(|s| s.key.elem).max().expect("stops");
        assert!(last_shown < 12, "the premise: the page shows only the first rows ({last_shown})");
        let mut rows_seen = 0;
        for step in 1..=40 {
            t.move_sel(1);
            for _ in 0..90 {
                t.update(1.0 / 60.0, frame.h);
            }
            let stops = stops_of(&t, frame, true, false);
            let mine = stops.iter().find(|s| s.key.elem == t.sel as u32).unwrap_or_else(|| panic!("step {step}: row {} has no stop", t.sel));
            let r = t.row_frame(frame, t.sel).unwrap();
            assert!(same(mine.rect, r) && same(mine.rest_rect, r), "step {step}: the stop is the row");
            assert!(hittable(mine), "step {step}: the focused row is on the page");
            rows_seen += 1;
        }
        assert_eq!(rows_seen, 40);
        assert!(t.scroll_pos() > 0.0, "the list scrolled");
    }

}
