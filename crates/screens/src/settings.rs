//! **Settings** — the tab destination and the family of pages under it, PlxNative's Settings
//! idiom: a route header beside a `FormTable`, every row focused by identity, every drill-down a
//! push on the shared stack whose crumb names where BACK returns.
//!
//! - **The root** (a tab page, under the top bar): *Account* — who is signed in and *Sign out*
//!   (`AccountCmd::SignOut`: the stored session is removed), or *Sign in*; *Playback* — subtitle
//!   size and position, each a picker; *Remote* — the key map; *About* — About StremioNative, Legal notices, Privacy, and
//!   the version line.
//! - **A picker** ([`SettingsPage::Picker`]): one checked row per rung; OK sets it (the
//!   application persists it and publishes it to the player) and returns.
//! - **Legal notices** ([`SettingsPage::Legal`]): the third-party components, and the full texts
//!   of the GPL, the fonts' OFL and the copyright notice — the package's own files, embedded so
//!   reading them never touches the disk on the frame thread.
//! - **A document** ([`SettingsPage::Doc`]): scrolls inside on UP/DOWN and leaves at its ends.
//!
//! The builders ([`root_form`], [`picker_form`], [`legal_form`], [`body`]) are pure, so the fit
//! tests and the navigation tests build the real pages.

use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::OnceLock;

use sn_machine::machine::{
    Canon, Cx, Edge, Effects, EntryId, FocusKey, Fx, GroupId, Handled, InputEvent, InputKind, Key, LogicalState, Machine,
    NavOp,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::document_reader::DocumentReader;
use sn_ui::form::{Activation, Form, FormId, FormSection, FormTable, RowKey, RowKind};
use sn_ui::route_screen::RouteLayout;
use sn_ui::screen::{
    At, Dir, DrawFrame, FocusSource, Focusable, GroupSpec, HitSource, Link, Part, Placed, RenderStrategy, Screen,
    ScreenEvent, Step,
};
use sn_ui::table::Row;
use sn_ui::table_screen::{DocumentFocus, DocumentScreen, Header, TableScreen};
use sn_ui::{theme, Rect};

use crate::registry::{word, AppArg, AppFx, Bundle, Doc, SettingsCmd, SettingsPage};
use crate::stores::account::{AccountCmd, AccountStore, SessionState};
use crate::stores::settings::{Pref, SettingsStore};
use crate::stores::StoreId;
use crate::tabs::Tab;

/// Every row key is below this.
const KEY_BAND: u32 = 0x100;
const GROUP: GroupId = GroupId(0);

/// The source address the GPL entitles every recipient to.
pub const SOURCE_URL: &str = "https://github.com/Morpheus235/stremio-native";
/// The project StremioNative is built on.
pub const PLXNATIVE_URL: &str = "https://github.com/GLinnik21/plx-native";

const GPL_TEXT: &str = include_str!("../../../LICENSE");
const OFL_TEXT: &str = include_str!("../../../pkg/OFL.txt");
const NOTICE_TEXT: &str = include_str!("../../../NOTICE");

/// The third-party components the package ships and their licences (`docs/legal.md`). Names and
/// licence identifiers are proper nouns, never translated.
pub const COMPONENTS: [(&str, &str); 5] = [
    ("FFmpeg", "LGPL-2.1-or-later"),
    ("libass, FreeType, HarfBuzz, FriBidi", "ISC; FreeType License; MIT; LGPL-2.1"),
    ("nanosvg", "zlib"),
    ("Inter, Noto (bundled fonts)", "SIL Open Font License 1.1"),
    ("Rust crates", "MIT, Apache-2.0, Unicode-3.0"),
];

static VERSION: OnceLock<&'static str> = OnceLock::new();

/// The version the About page and the root's version line show. The application hands it in at
/// boot (this crate cannot name the application, which knows its own version).
pub fn set_version(v: &'static str) {
    let _ = VERSION.set(v);
}

pub fn version() -> &'static str {
    VERSION.get().copied().unwrap_or("dev")
}

/// The root's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootRow {
    SubtitleSize,
    SubtitlePosition,
    RemoteKeys,
    About,
    Legal,
    Privacy,
    /// Signed out: opens the sign-in page.
    SignIn,
    /// Signed in: who it is. A stop so the remote can rest on it (and a long address scroll);
    /// OK does nothing.
    Account,
    /// Signed in: ends the session and removes it from the television.
    SignOut,
}

impl FormId for RootRow {
    fn key(&self) -> RowKey {
        RowKey(*self as u32 + 1)
    }
}

/// A picker's rung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung(pub u8);

impl FormId for Rung {
    fn key(&self) -> RowKey {
        RowKey(u32::from(self.0) + 1)
    }
}

/// The Legal index's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegalRow {
    Components,
    Gpl,
    Ofl,
    Notice,
}

impl FormId for LegalRow {
    fn key(&self) -> RowKey {
        RowKey(*self as u32 + 1)
    }
}

/// The label of rung `r` of `pref`.
pub fn rung_label(pref: Pref, r: u8) -> &'static str {
    match (pref, r) {
        (Pref::SubtitleSize, 0) => msg::app_settings_size_small(),
        (Pref::SubtitleSize, 1) => msg::app_settings_size_medium(),
        (Pref::SubtitleSize, 2) => msg::app_settings_size_large(),
        (Pref::SubtitleSize, 3) => msg::app_settings_size_extra_large(),
        (Pref::SubtitleSize, _) => msg::app_settings_size_huge(),
        (Pref::SubtitlePosition, 0) => msg::app_settings_position_low(),
        (Pref::SubtitlePosition, 1) => msg::app_settings_position_middle(),
        (Pref::SubtitlePosition, _) => msg::app_settings_position_high(),
    }
}

fn pref_label(pref: Pref) -> &'static str {
    match pref {
        Pref::SubtitleSize => msg::app_settings_subtitle_size(),
        Pref::SubtitlePosition => msg::app_settings_subtitle_position(),
    }
}

fn pref_copy(pref: Pref) -> &'static str {
    match pref {
        Pref::SubtitleSize => msg::app_settings_subtitle_size_copy(),
        Pref::SubtitlePosition => msg::app_settings_subtitle_position_copy(),
    }
}

/// The root's Account section: the session as the account store holds it.
fn account_section(account: &AccountStore) -> FormSection<RootRow, RootRow, SettingsPage> {
    let s = FormSection::new(msg::app_addons_account());
    match account.session() {
        SessionState::Restoring => s.note(msg::app_addons_restoring()),
        SessionState::SignedOut { .. } => s.item(
            RootRow::SignIn,
            RowKind::Button,
            RootRow::SignIn,
            Row::new(msg::app_addons_sign_in()).detail(msg::app_addons_sign_in_detail()).chevron(true),
        ),
        SessionState::SignedIn(a) => {
            // the account is user text: a long address elides, the label never does
            let who = if a.email.is_empty() { a.user_id.clone() } else { a.email.clone() };
            let s = s.item(RootRow::Account, RowKind::Button, RootRow::Account, Row::new(msg::app_addons_signed_in()).value(who).server_value());
            let s = if account.save_failed() { s.note(msg::app_addons_not_kept()) } else { s };
            s.item(RootRow::SignOut, RowKind::Button, RootRow::SignOut, Row::new(msg::app_addons_sign_out()).destructive(true))
        }
    }
}

/// The root's form, from the settings and the session as they stand.
pub fn root_form(settings: &SettingsStore, account: &AccountStore) -> Form<RootRow, RootRow, SettingsPage> {
    let nav = |p: SettingsPage| RowKind::Nav(p);
    Form::new()
        .section(account_section(account))
        .section(
            FormSection::new(msg::app_settings_playback())
                .item(
                    RootRow::SubtitleSize,
                    nav(SettingsPage::Picker(Pref::SubtitleSize)),
                    RootRow::SubtitleSize,
                    Row::new(pref_label(Pref::SubtitleSize)).value(rung_label(Pref::SubtitleSize, settings.get(Pref::SubtitleSize))),
                )
                .item(
                    RootRow::SubtitlePosition,
                    nav(SettingsPage::Picker(Pref::SubtitlePosition)),
                    RootRow::SubtitlePosition,
                    Row::new(pref_label(Pref::SubtitlePosition))
                        .value(rung_label(Pref::SubtitlePosition, settings.get(Pref::SubtitlePosition))),
                ),
        )
        .section(FormSection::new(msg::app_settings_remote()).item(
            RootRow::RemoteKeys,
            nav(SettingsPage::Doc(Doc::Remote)),
            RootRow::RemoteKeys,
            Row::new(msg::app_settings_remote_keys()).detail(msg::app_settings_remote_keys_detail()),
        ))
        .section(
            FormSection::new(msg::app_settings_about_section())
                .item(RootRow::About, nav(SettingsPage::Doc(Doc::About)), RootRow::About, Row::new(msg::app_settings_about()))
                .item(RootRow::Legal, nav(SettingsPage::Legal), RootRow::Legal, Row::new(msg::app_settings_legal()))
                .item(RootRow::Privacy, nav(SettingsPage::Doc(Doc::Privacy)), RootRow::Privacy, Row::new(msg::app_settings_privacy()))
                .note(msg::app_settings_version(version())),
        )
}

/// A picker's form: one checked row per rung.
pub fn picker_form(pref: Pref, current: u8) -> Form<Rung, Rung, Infallible> {
    let mut s = FormSection::new(pref_label(pref));
    for r in 0..pref.rungs() {
        s = s.choice(Rung(r), Rung(r), Row::new(rung_label(pref, r)), |id| id.0 == current);
    }
    Form::new().section(s)
}

/// The Legal index's form.
pub fn legal_form() -> Form<LegalRow, LegalRow, SettingsPage> {
    Form::new().section(
        FormSection::new(msg::app_settings_legal())
            .item(LegalRow::Components, RowKind::Nav(SettingsPage::Doc(Doc::Components)), LegalRow::Components, Row::new(doc_title(Doc::Components)))
            .item(LegalRow::Gpl, RowKind::Nav(SettingsPage::Doc(Doc::Gpl)), LegalRow::Gpl, Row::new(doc_title(Doc::Gpl)))
            .item(LegalRow::Ofl, RowKind::Nav(SettingsPage::Doc(Doc::Ofl)), LegalRow::Ofl, Row::new(doc_title(Doc::Ofl)))
            .item(LegalRow::Notice, RowKind::Nav(SettingsPage::Doc(Doc::Notice)), LegalRow::Notice, Row::new(doc_title(Doc::Notice))),
    )
}

pub fn doc_title(doc: Doc) -> &'static str {
    match doc {
        Doc::Remote => msg::app_settings_remote_keys(),
        Doc::About => msg::app_settings_about(),
        Doc::Privacy => msg::app_settings_privacy(),
        Doc::Components => msg::app_settings_components(),
        Doc::Gpl => msg::app_settings_gpl(),
        Doc::Ofl => msg::app_settings_ofl(),
        Doc::Notice => msg::app_settings_notice(),
    }
}

fn doc_copy(doc: Doc) -> String {
    match doc {
        Doc::Remote => msg::app_remote_copy().to_string(),
        Doc::About => msg::app_settings_version(version()),
        Doc::Privacy => msg::app_privacy_copy().to_string(),
        Doc::Components | Doc::Gpl | Doc::Ofl | Doc::Notice => String::new(),
    }
}

/// A document's text. Headings are ALL-CAPS lines (the reader draws them bold); paragraphs are
/// separated by a blank line.
pub fn body(doc: Doc) -> String {
    match doc {
        Doc::Remote => remote_body(),
        Doc::About => [
            msg::app_about_affiliation_head().to_uppercase(),
            msg::app_about_affiliation().to_string(),
            String::new(),
            msg::app_about_content_head().to_uppercase(),
            msg::app_about_content().to_string(),
            String::new(),
            msg::app_about_licence_head().to_uppercase(),
            msg::app_about_licence().to_string(),
            SOURCE_URL.to_string(),
            String::new(),
            msg::app_about_plxnative_head().to_uppercase(),
            msg::app_about_plxnative().to_string(),
            PLXNATIVE_URL.to_string(),
            String::new(),
            msg::app_about_version_head().to_uppercase(),
            version().to_string(),
        ]
        .join("\n"),
        Doc::Privacy => [
            msg::app_privacy_collects().to_string(),
            String::new(),
            msg::app_privacy_leaves_head().to_uppercase(),
            msg::app_privacy_sign_in().to_string(),
            msg::app_privacy_addons().to_string(),
            msg::app_privacy_media().to_string(),
            msg::app_privacy_artwork().to_string(),
            String::new(),
            msg::app_privacy_stays_head().to_uppercase(),
            msg::app_privacy_stays().to_string(),
            String::new(),
            msg::app_privacy_full(&format!("{SOURCE_URL}/blob/main/PRIVACY.md")),
        ]
        .join("\n"),
        Doc::Components => {
            let mut lines = vec![msg::app_settings_components_intro().to_string(), String::new()];
            lines.extend(COMPONENTS.iter().map(|(c, l)| format!("{c} \u{2014} {l}")));
            lines.push(String::new());
            lines.push(msg::app_settings_components_texts().to_string());
            lines.join("\n")
        }
        Doc::Gpl => GPL_TEXT.to_string(),
        Doc::Ofl => OFL_TEXT.to_string(),
        Doc::Notice => NOTICE_TEXT.to_string(),
    }
}

/// The key map in the viewer's words — `docs/remote-keys.md` is the engineering half.
fn remote_body() -> String {
    let line = |key: &str, what: &str| format!("{key} \u{2014} {what}");
    [
        msg::app_remote_navigation_head().to_uppercase(),
        line(msg::app_remote_key_arrows(), msg::app_remote_move()),
        line(msg::app_remote_key_ok(), msg::app_remote_select()),
        line(msg::app_remote_key_hold_ok(), msg::app_remote_options()),
        line(msg::app_remote_key_back(), msg::app_remote_back()),
        line(msg::app_remote_key_exit(), msg::app_remote_exit()),
        String::new(),
        msg::app_remote_shortcuts_head().to_uppercase(),
        msg::app_remote_shortcuts_where().to_string(),
        line(msg::app_remote_key_red(), msg::app_remote_prev_tab()),
        line(msg::app_remote_key_green(), msg::app_remote_next_tab()),
        line(msg::app_remote_key_yellow(), msg::app_tab_settings()),
        line(msg::app_remote_key_blue(), msg::app_tab_search()),
        line(msg::app_remote_key_ch_up(), msg::app_remote_prev_tab()),
        line(msg::app_remote_key_ch_down(), msg::app_remote_next_tab()),
        String::new(),
        msg::app_remote_playback_head().to_uppercase(),
        line(msg::app_remote_key_play(), msg::app_remote_pause()),
        line(msg::app_remote_key_stop(), msg::app_remote_stop()),
    ]
    .join("\n")
}

enum Mode {
    Root(FormTable<RootRow, RootRow, SettingsPage>),
    Picker(Pref, FormTable<Rung, Rung, Infallible>),
    Legal(FormTable<LegalRow, LegalRow, SettingsPage>),
    Doc { doc: Doc, reader: DocumentReader, body: String, copy: String },
}

pub struct SettingsScreen {
    entry: EntryId,
    mode: Mode,
    /// The settings and account generations the root and the picker were built from.
    built: Option<(u32, u32)>,
}

impl SettingsScreen {
    /// The tab destination.
    pub fn root(entry: EntryId) -> Self {
        SettingsScreen { entry, mode: Mode::Root(FormTable::new(KEY_BAND)), built: None }
    }

    /// A page of the family.
    pub fn page(entry: EntryId, page: SettingsPage) -> Self {
        let mode = match page {
            SettingsPage::Picker(p) => Mode::Picker(p, FormTable::new(KEY_BAND)),
            SettingsPage::Legal => {
                let mut f = FormTable::new(KEY_BAND);
                f.set(legal_form(), None);
                Mode::Legal(f)
            }
            SettingsPage::Doc(doc) => Mode::Doc { doc, reader: DocumentReader::new(), body: body(doc), copy: doc_copy(doc) },
        };
        SettingsScreen { entry, mode, built: None }
    }

    fn is_root(&self) -> bool {
        matches!(self.mode, Mode::Root(_))
    }

    /// The page's rows as the tests read them (`None` on a document).
    pub fn table(&self) -> Option<&sn_ui::table::TableView> {
        match &self.mode {
            Mode::Root(f) => Some(&f.table),
            Mode::Picker(_, f) => Some(&f.table),
            Mode::Legal(f) => Some(&f.table),
            Mode::Doc { .. } => None,
        }
    }

    /// A document page's text (`None` on a table page).
    pub fn document(&self) -> Option<&str> {
        match &self.mode {
            Mode::Doc { body, .. } => Some(body),
            _ => None,
        }
    }

    fn layout(&self) -> RouteLayout {
        if self.is_root() {
            RouteLayout::below_top_bar()
        } else {
            RouteLayout::screen()
        }
    }

    fn crumb(&self) -> Option<&'static str> {
        match &self.mode {
            Mode::Root(_) => None,
            Mode::Doc { doc: Doc::Components | Doc::Gpl | Doc::Ofl | Doc::Notice, .. } => Some(msg::app_settings_legal()),
            _ => Some(msg::app_tab_settings()),
        }
    }

    fn title(&self) -> &'static str {
        match &self.mode {
            Mode::Root(_) => msg::app_tab_settings(),
            Mode::Picker(p, _) => pref_label(*p),
            Mode::Legal(_) => msg::app_settings_legal(),
            Mode::Doc { doc, .. } => doc_title(*doc),
        }
    }

    fn copy(&self) -> &str {
        match &self.mode {
            Mode::Root(_) => msg::app_settings_copy(),
            Mode::Picker(p, _) => pref_copy(*p),
            Mode::Legal(_) => msg::app_settings_legal_copy(),
            Mode::Doc { copy, .. } => copy,
        }
    }

    fn doc_frame(&self) -> Rect {
        self.layout().document(self.crumb().is_some())
    }

    /// Rebuild the root / the picker when the settings or the session moved.
    fn sync(&mut self, store: &SettingsStore, account: &AccountStore) {
        let gen = (store.gen(), account.gen());
        if self.built == Some(gen) {
            return;
        }
        let fresh = self.built.is_none();
        self.built = Some(gen);
        match &mut self.mode {
            Mode::Root(f) => {
                if fresh {
                    f.set(root_form(store, account), None);
                } else {
                    f.refresh(root_form(store, account));
                }
            }
            Mode::Picker(p, f) => {
                let current = store.get(*p);
                if fresh {
                    f.set(picker_form(*p, current), Some(&Rung(current)));
                } else {
                    f.refresh(picker_form(*p, current));
                }
            }
            Mode::Legal(_) | Mode::Doc { .. } => {}
        }
    }

    fn table_view(&self) -> Option<TableScreen<'_>> {
        let header = Header::new(self.layout(), self.crumb(), self.title(), self.copy());
        let v = match &self.mode {
            Mode::Root(f) => TableScreen::new(header, &f.table, GROUP, self.entry).keyed(f),
            Mode::Picker(_, f) => TableScreen::new(header, &f.table, GROUP, self.entry).keyed(f),
            Mode::Legal(f) => TableScreen::new(header, &f.table, GROUP, self.entry).keyed(f),
            Mode::Doc { .. } => return None,
        };
        Some(v)
    }

    fn with_focus<R>(&self, f: impl FnOnce(&dyn Focusable<Bundle>) -> R) -> R {
        match &self.mode {
            Mode::Doc { reader, .. } => f(&DocumentFocus { reader, frame: self.doc_frame(), group: GROUP, entry: self.entry }),
            _ => f(&self.table_view().expect("a table page")),
        }
    }

    fn note_focus(&mut self, elem: u32) {
        let key = RowKey(elem);
        match &mut self.mode {
            Mode::Root(f) => focus_row(f, key),
            Mode::Picker(_, f) => focus_row(f, key),
            Mode::Legal(f) => focus_row(f, key),
            Mode::Doc { .. } => {}
        }
    }

    fn activate(&mut self, elem: u32, fx: &mut Effects<'_, Bundle>) {
        let key = RowKey(elem);
        match &self.mode {
            Mode::Root(f) => match f.index_of_key(key).and_then(|i| f.activate(i)) {
                Some(Activation::Push(page)) => fx.push(Fx::Nav(NavOp::Push(AppArg::SettingsPage(page)))),
                Some(Activation::Action(RootRow::SignIn)) => fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn))),
                Some(Activation::Action(RootRow::SignOut)) => {
                    fx.push(Fx::App(AppFx::Account(AccountCmd::SignOut)));
                    fx.invalidate(Provenance::Input);
                }
                Some(Activation::Action(_)) | None => {}
            },
            Mode::Legal(f) => {
                if let Some(Activation::Push(page)) = f.index_of_key(key).and_then(|i| f.activate(i)) {
                    fx.push(Fx::Nav(NavOp::Push(AppArg::SettingsPage(page))));
                }
            }
            Mode::Picker(p, f) => {
                if let Some(Activation::Action(Rung(r))) = f.index_of_key(key).and_then(|i| f.activate(i)) {
                    fx.push(Fx::App(AppFx::Settings(SettingsCmd::Set(*p, r))));
                    fx.push(Fx::Nav(NavOp::Pop));
                }
            }
            Mode::Doc { .. } => {}
        }
    }

    fn tick(&mut self, dt: f32) {
        let h = self.layout().sectioned_table().h;
        match &mut self.mode {
            Mode::Root(f) => f.table.update(dt, h),
            Mode::Picker(_, f) => f.table.update(dt, h),
            Mode::Legal(f) => f.table.update(dt, h),
            Mode::Doc { reader, .. } => reader.update(dt),
        }
    }
}

fn focus_row<Id: PartialEq + Clone, A: Clone, D: Clone>(f: &mut FormTable<Id, A, D>, key: RowKey) {
    if let Some(i) = f.index_of_key(key) {
        f.table.sel = i as i32;
    }
    f.note_engine_key(Some(key));
}

impl LogicalState for SettingsScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
        match &self.mode {
            Mode::Root(f) => {
                c.u32(0).u32(f.table.sel as u32);
            }
            Mode::Picker(p, f) => {
                c.u32(1).u32(*p as u32).u32(f.table.sel as u32);
            }
            Mode::Legal(f) => {
                c.u32(2).u32(f.table.sel as u32);
            }
            Mode::Doc { doc, .. } => {
                c.u32(3).u32(*doc as u32);
            }
        }
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("settings {}", self.title()));
    }
}

impl Machine<Bundle> for SettingsScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                self.sync(cx.views.settings, cx.views.account);
                // a picker opens on the rung in force, not on its first row
                if let Mode::Picker(p, _) = &self.mode {
                    let key = Rung(cx.views.settings.get(*p)).key();
                    crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(FocusKey { entry: self.entry, elem: key.0 }), fx);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Settings.ord() || *ord == StoreId::Account.ord() => {
                self.sync(cx.views.settings, cx.views.account);
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.sync(cx.views.settings, cx.views.account);
                self.tick(t.dt());
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                self.note_focus(to.elem);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Settings, Some(GROUP), fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.activate(*elem, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                    if crate::tabs::on_strip(k.elem) {
                        crate::tabs::on_pill(k.elem, Tab::Settings, Some(GROUP), fx);
                    } else {
                        self.activate(k.elem, fx);
                    }
                }
                Handled::Yes
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: Key::Back, edge: Edge::Down, .. }, .. }) if self.is_root() => {
                crate::tabs::on_back(self.entry, Tab::Settings, cx, fx)
            }
            ScreenEvent::Input(InputEvent { kind: InputKind::Key { key: key @ (Key::Up | Key::Down), edge, .. }, .. })
                if *edge != Edge::Up =>
            {
                // a document scrolls INSIDE on UP/DOWN and leaves at its ends (the engine answers
                // `Edge` there)
                let Mode::Doc { reader, .. } = &mut self.mode else { return Handled::No };
                let inside = if *key == Key::Up { !reader.at_top() } else { !reader.at_end() };
                if inside {
                    reader.move_by(if *key == Key::Up { -1 } else { 1 });
                    fx.invalidate(Provenance::Input);
                    Handled::Yes
                } else {
                    Handled::No
                }
            }
            ScreenEvent::Enter(_) | ScreenEvent::Uncover => {
                fx.invalidate(Provenance::Nav);
                Handled::Yes
            }
            _ => Handled::No,
        }
    }
}

impl Focusable<Bundle> for SettingsScreen {
    fn groups(&self, cx: &Cx<'_, Bundle>, out: &mut Vec<GroupSpec>) {
        self.with_focus(|v| v.groups(cx, out))
    }
    fn group_of(&self, key: &u32, cx: &Cx<'_, Bundle>) -> Option<GroupId> {
        self.with_focus(|v| v.group_of(key, cx))
    }
    fn neighbour(&self, key: FocusKey<u32>, dir: Dir, cx: &Cx<'_, Bundle>) -> Step<u32> {
        self.with_focus(|v| v.neighbour(key, dir, cx))
    }
    fn place(&self, key: &u32, cx: &Cx<'_, Bundle>, at: At) -> Option<Placed> {
        self.with_focus(|v| v.place(key, cx, at))
    }
    fn reconcile(&self, want: FocusKey<u32>, cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        self.with_focus(|v| v.reconcile(want, cx))
    }
    fn seat(&self, g: GroupId, from: Placed, cx: &Cx<'_, Bundle>) -> FocusKey<u32> {
        self.with_focus(|v| v.seat(g, from, cx))
    }
}

impl Screen<Bundle> for SettingsScreen {
    fn name(&self) -> &'static str {
        if self.is_root() {
            word::SETTINGS
        } else {
            word::SETTINGS_PAGE
        }
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        self.crumb().map(Cow::Borrowed)
    }
    fn links(&self, out: &mut Vec<Link>) {
        if self.is_root() {
            crate::tabs::links(GROUP, out);
        }
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, _cx: &Cx<'_, Bundle>) {}
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        let layout = self.layout();
        let crumb = self.crumb();
        let title = self.title();
        let doc_frame = self.doc_frame();
        let entry = self.entry;
        if let Mode::Doc { reader, body, copy, .. } = &mut self.mode {
            let mut v = DocumentScreen::new(Header::new(layout, crumb, title, copy), reader, body, GROUP, entry);
            v.doc.frame = doc_frame;
            Part::<Bundle>::draw(&mut v, f, Rect::FULL);
            return;
        }
        if let Some(mut v) = self.table_view() {
            Part::<Bundle>::draw(&mut v, f, Rect::FULL);
        }
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
    use sn_platform::i18n::{language_on_this_thread_for_test, SHIPPED};

    fn signed_in(email: &str) -> AccountStore {
        use crate::stores::account::{Account, AccountLanding, Secret, Stored};
        let mut a = AccountStore::new();
        let account = Account { user_id: "u1".into(), email: email.into(), auth_key: Secret::new("k") };
        let _ = a.land(AccountLanding::Restored(Stored::Session(account)), 0);
        a
    }

    /// The root's account section in each of its states.
    fn accounts() -> Vec<(&'static str, AccountStore)> {
        vec![
            ("restoring", AccountStore::new()),
            ("signed-out", AccountStore::signed_out(true)),
            ("signed-in", signed_in("someone.with.a.long.address@example.com")),
        ]
    }

    fn fit(tag: &str, table: &sn_ui::table::TableView, layout: RouteLayout, out: &mut Vec<String>) {
        out.extend(table.app_fit_failures(layout.sectioned_table().w, tag));
    }

    /// Every app-owned row on every table page of the family fits its column, in every shipped
    /// language, at every rung of every preference (the root's trailing values change with them).
    #[test]
    fn every_settings_row_fits_its_column_in_every_shipped_language() {
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let tag = language.tag();
            for size in 0..Pref::SubtitleSize.rungs() {
                for position in 0..Pref::SubtitlePosition.rungs() {
                    for (who, account) in accounts() {
                        let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
                        f.set(root_form(&SettingsStore::with(size, position), &account), None);
                        let tag = format!("{tag} root {who} size={size} position={position}");
                        fit(&tag, &f.table, RouteLayout::below_top_bar(), &mut out);
                    }
                }
            }
            for pref in Pref::ALL {
                let mut f = FormTable::<Rung, Rung, Infallible>::new(KEY_BAND);
                f.set(picker_form(pref, pref.default_rung()), None);
                fit(&format!("{tag} picker {pref:?}"), &f.table, RouteLayout::screen(), &mut out);
            }
            let mut f = FormTable::<LegalRow, LegalRow, SettingsPage>::new(KEY_BAND);
            f.set(legal_form(), None);
            fit(&format!("{tag} legal"), &f.table, RouteLayout::screen(), &mut out);
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    #[test]
    fn the_about_page_names_the_source_the_credit_and_the_affiliation() {
        let about = body(Doc::About);
        assert!(about.contains(SOURCE_URL), "the GPL source address");
        assert!(about.contains(PLXNATIVE_URL), "the PlxNative credit");
        assert!(about.contains(msg::app_about_affiliation()));
        assert!(about.contains(version()));
        assert!(body(Doc::Gpl).contains("GNU GENERAL PUBLIC LICENSE"));
        assert!(body(Doc::Ofl).contains("SIL Open Font License"));
        assert!(body(Doc::Notice).contains("PlxNative"));
        let components = body(Doc::Components);
        assert!(COMPONENTS.iter().all(|(c, l)| components.contains(c) && components.contains(l)));
    }

    #[test]
    fn the_remote_page_lists_every_hotkey() {
        let remote = body(Doc::Remote);
        for key in [
            msg::app_remote_key_red(),
            msg::app_remote_key_green(),
            msg::app_remote_key_yellow(),
            msg::app_remote_key_blue(),
            msg::app_remote_key_ch_up(),
            msg::app_remote_key_ch_down(),
        ] {
            assert!(remote.contains(key), "{key} is on the page");
        }
    }

    #[test]
    fn every_root_row_opens_its_own_page() {
        let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
        f.set(root_form(&SettingsStore::default(), &AccountStore::signed_out(true)), None);
        let pushed: Vec<SettingsPage> = (0..f.table.n_rows() as usize)
            .filter_map(|i| match f.activate(i) {
                Some(Activation::Push(p)) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(
            pushed,
            vec![
                SettingsPage::Picker(Pref::SubtitleSize),
                SettingsPage::Picker(Pref::SubtitlePosition),
                SettingsPage::Doc(Doc::Remote),
                SettingsPage::Doc(Doc::About),
                SettingsPage::Legal,
                SettingsPage::Doc(Doc::Privacy),
            ]
        );
    }

    #[test]
    fn the_account_section_signs_in_or_out() {
        let actions = |account: &AccountStore| -> Vec<RootRow> {
            let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
            f.set(root_form(&SettingsStore::default(), account), None);
            (0..f.table.n_rows() as usize)
                .filter_map(|i| match f.activate(i) {
                    Some(Activation::Action(a)) => Some(a),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(actions(&AccountStore::signed_out(false)), vec![RootRow::SignIn]);
        assert_eq!(actions(&signed_in("a@b.c")), vec![RootRow::Account, RootRow::SignOut]);
        assert_eq!(actions(&AccountStore::new()), Vec::<RootRow>::new(), "nothing to press while the session is read");
        let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
        f.set(root_form(&SettingsStore::default(), &signed_in("a@b.c")), None);
        let rows: Vec<&sn_ui::table::Row> = f.table.sections.iter().flat_map(|s| s.rows.iter()).collect();
        assert!(rows.iter().any(|r| r.value.as_deref() == Some("a@b.c")), "the account is shown");
    }
}
