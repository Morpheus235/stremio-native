//! **Sign in** — to a Stremio account, two ways, on one route page (PlxNative's sign-in screen,
//! re-cut for Stremio's two flows):
//!
//! - **With a phone** (the narrative column): the device link. The page asks for a code as it
//!   opens, shows it with a QR of its link page (`link.stremio.com`), and reads it until the
//!   account behind it answers (`AccountCmd::PollLink` on the tick). An expired or failed code
//!   offers a new one.
//! - **With email and password** (the table): two text rows typed on the television's own keyboard
//!   (OK on a row raises it, `Delivery::Keyboard`; the panel's commits arrive as
//!   `InputKind::Text`, its edit keys as Backspace / Clear / ◀ ▶), then *Sign in*. The attempt's
//!   read-out sits under the button: working, the API's refusal in its own words, or a network
//!   failure with *Try again*.
//! - **Continue without signing in**: Cinemeta only, and the first-run question is not asked again.
//!
//! The page leaves itself (a nav pop) once the account is signed in, or the viewer chose to go on
//! without one. The table is built by [`build`], a pure function of the account store and the
//! typed fields, so the read-outs are graded by the host suite; the typed password is masked
//! everywhere it is drawn and is never logged.

use std::borrow::Cow;

use sn_machine::machine::{
    Canon, Cx, Delivery, Edge, Effects, EntryId, Fx, GroupId, Handled, InputKind, Key, LogicalState, Machine, NavOp, TextEdit,
};
use sn_machine::present::Provenance;
use sn_platform::i18n::msg;
use sn_ui::form::{RowKey, RowKeys};
use sn_ui::qr::{QrCode, QrLink};
use sn_ui::route_screen::RouteLayout;
use sn_ui::screen::{DrawFrame, FocusSource, HitSource, Part, RenderStrategy, Screen, ScreenEvent};
use sn_ui::table::{Row, Section, TableView};
use sn_ui::table_screen::{Header, TableScreen};
use sn_ui::text_buffer::TextBuffer;
use sn_ui::{theme, Rect};

use crate::registry::{word, AppFx, Bundle, FirstSeat};
use crate::stores::account::{AccountCmd, AccountStore, Link, Secret, SessionState, SignIn};
use crate::stores::StoreId;

/// The table's row keys.
pub const LINK_ACTION: u32 = 1;
pub const EMAIL: u32 = 2;
pub const PASSWORD: u32 = 3;
pub const SUBMIT: u32 = 4;
pub const RETRY: u32 = 5;
pub const SKIP: u32 = 6;
const NOTE: u32 = 0x100;

/// The address shown under the QR, for typing on a computer.
pub const LINK_ADDRESS: &str = "link.stremio.com";

/// Which text row the keyboard is typing into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Email,
    Password,
}

/// The masked form of a password, one dot per character.
pub fn masked(s: &str) -> String {
    "\u{2022}".repeat(s.chars().count())
}

/// The table and, per global row, its key.
#[derive(Default)]
pub struct Built {
    pub sections: Vec<Section>,
    pub keys: Vec<RowKey>,
}

impl Built {
    fn push(&mut self, s: &mut Section, row: Row, key: u32) {
        s.rows.push(row);
        self.keys.push(RowKey(key));
    }
    fn note(&mut self, s: &mut Section, text: impl Into<String>) {
        let key = NOTE + self.keys.len() as u32;
        self.push(s, Row::note(text), key);
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
    fn reseat(&self) -> Option<RowKey> {
        None
    }
}

/// A text row's trailing value: what was typed (with the caret while typing), else a prompt.
fn field_value(text: &str, editing: bool, caret: usize, mask: bool) -> (String, bool) {
    let shown = if mask { masked(text) } else { text.to_string() };
    if editing {
        // the caret, at its place in the shown text
        let at = if mask { text[..caret.min(text.len())].chars().count() * '\u{2022}'.len_utf8() } else { caret.min(shown.len()) };
        let mut s = shown;
        s.insert(at, '|');
        return (s, false);
    }
    if text.is_empty() {
        (msg::app_signin_not_entered().to_string(), true)
    } else {
        (shown, false)
    }
}

/// The sign-in table: the email form, its read-out, and the way out.
pub fn build(store: &AccountStore, email: &TextBuffer, password: &TextBuffer, editing: Option<Field>) -> Built {
    let mut out = Built::default();
    let mut form = Section::new(msg::app_signin_with_email());
    let (value, dim) = field_value(email.text(), editing == Some(Field::Email), email.caret(), false);
    let mut row = Row::new(msg::app_signin_email()).value(value).value_dim(dim);
    if !email.text().is_empty() {
        row = row.server_value();
    }
    out.push(&mut form, row, EMAIL);
    let (value, dim) = field_value(password.text(), editing == Some(Field::Password), password.caret(), true);
    out.push(&mut form, Row::new(msg::app_signin_password()).value(value).value_dim(dim), PASSWORD);
    let ready = !email.text().trim().is_empty() && !password.text().is_empty();
    let working = matches!(store.sign_in(), SignIn::Working);
    out.push(&mut form, Row::new(msg::app_signin_submit()).chevron(true).dim(!ready || working), SUBMIT);
    match store.sign_in() {
        SignIn::Idle => {}
        SignIn::Working => out.note(&mut form, msg::app_signin_working()),
        SignIn::Rejected(why) => out.note(&mut form, msg::app_signin_rejected(why)),
        SignIn::Offline(why) => {
            out.note(&mut form, msg::app_signin_offline(why));
            out.push(&mut form, Row::new(msg::app_signin_retry()), RETRY);
        }
    }
    out.sections.push(form);
    let mut phone = Section::new(msg::app_signin_with_phone());
    match store.link() {
        Link::Off => out.push(&mut phone, Row::new(msg::app_signin_link_start()), LINK_ACTION),
        Link::Creating => out.note(&mut phone, msg::app_signin_link_creating()),
        Link::Waiting { .. } => out.note(&mut phone, msg::app_signin_link_waiting()),
        Link::Confirming => out.note(&mut phone, msg::app_signin_working()),
        Link::Failed(why) => {
            out.note(&mut phone, msg::app_signin_link_failed(why));
            out.push(&mut phone, Row::new(msg::app_signin_retry()), LINK_ACTION);
        }
        Link::Expired => {
            out.note(&mut phone, msg::app_signin_link_expired());
            out.push(&mut phone, Row::new(msg::app_signin_link_new()), LINK_ACTION);
        }
    }
    out.sections.push(phone);
    let mut rest = Section::new(msg::app_signin_without());
    out.push(&mut rest, Row::new(msg::app_signin_skip()).detail(msg::app_signin_skip_detail()), SKIP);
    out.sections.push(rest);
    out
}

pub struct SignInScreen {
    entry: EntryId,
    table: TableView,
    built: Built,
    built_gen: Option<(u32, u64)>,
    sel: Option<RowKey>,
    email: TextBuffer,
    password: TextBuffer,
    /// The field the television's keyboard is typing into.
    editing: Option<Field>,
    /// The edit generation, so a keystroke rebuilds the table.
    edits: u64,
    /// The QR of the shown code, encoded when the code arrives.
    qr: Option<(String, QrCode)>,
    /// The page started a sign-in (so a SignedIn store means this page's work is done).
    started: bool,
    left: bool,
    seat: FirstSeat,
}

impl SignInScreen {
    pub fn new(entry: EntryId) -> Self {
        let mut table = TableView::new();
        table.list_focused = true;
        SignInScreen {
            entry,
            table,
            built: Built::default(),
            built_gen: None,
            sel: None,
            email: TextBuffer::new(String::new(), 0),
            password: TextBuffer::new(String::new(), 0),
            editing: None,
            edits: 0,
            qr: None,
            started: false,
            left: false,
            seat: FirstSeat::new(),
        }
    }

    pub fn editing(&self) -> Option<Field> {
        self.editing
    }

    /// The table as the test suite reads it.
    pub fn sections(&self) -> &[Section] {
        &self.table.sections
    }

    fn layout() -> RouteLayout {
        RouteLayout::screen()
    }

    fn view(&self) -> TableScreen<'_> {
        TableScreen::new(
            Header::new(Self::layout(), Some(msg::app_nav_home()), msg::app_signin_title(), msg::app_signin_copy()),
            &self.table,
            GroupId(0),
            self.entry,
        )
        .keyed(&self.built)
        .uncommitted(self.editing.is_some())
    }

    fn sync(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let store = cx.views.account;
        // done: signed in (by this page or another way), or the viewer went on without
        if !self.left {
            let done = match store.session() {
                SessionState::SignedIn(_) => true,
                SessionState::SignedOut { skipped: true } => self.started,
                _ => false,
            };
            if done {
                self.left = true;
                self.keyboard(None, fx);
                fx.push(Fx::Nav(NavOp::Pop));
            }
        }
        // the QR follows the shown code
        let shown = match store.link() {
            Link::Waiting { code, .. } => Some(code.link.clone()),
            _ => None,
        };
        if self.qr.as_ref().map(|(l, _)| l) != shown.as_ref() {
            self.qr = shown.and_then(|l| QrCode::new(&l).ok().map(|q| (l, q)));
        }
        let gen = (store.gen(), self.edits);
        if self.built_gen == Some(gen) {
            return;
        }
        self.built_gen = Some(gen);
        let built = build(store, &self.email, &self.password, self.editing);
        let sel = self.sel.and_then(|k| built.index_of(k)).map(|i| i as i32);
        let fresh = self.table.sections.is_empty();
        self.built = built;
        let sections = std::mem::take(&mut self.built.sections);
        self.table.set_sections_or_open(sections, sel, !fresh);
        fx.invalidate(Provenance::Landing(fx.from()));
    }

    /// Raise the keyboard for `field`, or put it away (`None`).
    fn keyboard(&mut self, field: Option<Field>, fx: &mut Effects<'_, Bundle>) {
        if self.editing == field {
            return;
        }
        let was = self.editing.is_some();
        self.editing = field;
        self.edits += 1;
        if was != field.is_some() {
            let me = fx.from();
            fx.push(Fx::Deliver(me, Delivery::Keyboard { up: field.is_some() }));
        }
        fx.invalidate(Provenance::Input);
    }

    fn edit(&mut self, edit: &TextEdit) {
        let buffer = match self.editing {
            Some(Field::Email) => &mut self.email,
            Some(Field::Password) => &mut self.password,
            None => return,
        };
        buffer.edit(edit);
        self.edits += 1;
    }

    fn submit(&mut self, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        if self.email.text().trim().is_empty() || self.password.text().is_empty() || matches!(cx.views.account.sign_in(), SignIn::Working) {
            return;
        }
        self.keyboard(None, fx);
        self.started = true;
        fx.push(Fx::App(AppFx::Account(AccountCmd::SignIn {
            email: self.email.text().to_string(),
            password: Secret::new(self.password.text()),
        })));
    }

    fn activate(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        match elem {
            EMAIL => self.keyboard(Some(Field::Email), fx),
            PASSWORD => self.keyboard(Some(Field::Password), fx),
            SUBMIT => self.submit(cx, fx),
            RETRY => {
                self.started = true;
                fx.push(Fx::App(AppFx::Account(AccountCmd::Retry)));
            }
            LINK_ACTION => fx.push(Fx::App(AppFx::Account(AccountCmd::StartLink))),
            SKIP => {
                self.started = true;
                self.keyboard(None, fx);
                fx.push(Fx::App(AppFx::Account(AccountCmd::ContinueSignedOut)));
            }
            _ => {}
        }
    }

    /// A key while the keyboard is up: the panel's edit keys, OK to finish the field, BACK to put
    /// the panel away. `None` for a key the page does not take.
    fn editing_key(&mut self, key: Key, sym: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Option<()> {
        let field = self.editing?;
        let edit = match (key, sym) {
            (Key::Left, _) => Some(TextEdit::Left),
            (Key::Right, _) => Some(TextEdit::Right),
            (_, sn_ui::consts::SDLK_BACKSPACE) => Some(TextEdit::Backspace),
            (_, sn_ui::consts::SDLK_CLEAR) => Some(TextEdit::Clear),
            _ => None,
        };
        if let Some(edit) = edit {
            self.edit(&edit);
            fx.invalidate(Provenance::Input);
            return Some(());
        }
        match key {
            Key::Back => self.keyboard(None, fx),
            Key::Ok => match field {
                // OK on the email goes on to the password; on the password, signs in
                Field::Email => {
                    self.keyboard(Some(Field::Password), fx);
                    self.sel = Some(RowKey(PASSWORD));
                    fx.push(Fx::Deliver(
                        fx.from(),
                        Delivery::Screen(ScreenEvent::Enter(sn_ui::screen::Enter::Fresh {
                            focus: sn_ui::screen::FocusTarget::Elem(sn_machine::machine::FocusKey { entry: self.entry, elem: PASSWORD }),
                        })),
                    ));
                }
                Field::Password => {
                    self.keyboard(None, fx);
                    self.submit(cx, fx);
                }
            },
            _ => return None,
        }
        Some(())
    }
}

impl LogicalState for SignInScreen {
    fn write(&self, c: &mut Canon) {
        c.u32(self.entry.0);
        c.option(self.sel, |c, k| {
            c.u32(k.0);
        });
        c.u32(match self.editing {
            None => 0,
            Some(Field::Email) => 1,
            Some(Field::Password) => 2,
        });
        // the lengths, never the text: the password must not reach a recording
        c.u32(self.email.text().len() as u32).u32(self.password.text().chars().count() as u32);
        self.table.write_motion(c);
    }
    fn probe(&self, out: &mut String) {
        out.push_str(&format!("signin rows={} editing={:?}", self.table.n_rows(), self.editing));
    }
}

impl Machine<Bundle> for SignInScreen {
    type Ev = ScreenEvent<Bundle>;
    fn step(&mut self, ev: &Self::Ev, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) -> Handled {
        match ev {
            ScreenEvent::Mount => {
                // the phone code is asked for at once: it is the quickest way in
                if matches!(cx.views.account.link(), Link::Off | Link::Expired | Link::Failed(_)) {
                    fx.push(Fx::App(AppFx::Account(AccountCmd::StartLink)));
                }
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Account.ord() => {
                self.sync(cx, fx);
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                if matches!(cx.views.account.link(), Link::Waiting { .. }) {
                    fx.push(Fx::App(AppFx::Account(AccountCmd::PollLink { now_ms: cx.tick.ms })));
                }
                self.sync(cx, fx);
                let target = Some(sn_ui::screen::FocusTarget::FirstInGroup(GroupId(0)));
                self.seat.tick(cx.focus.current.is_some_and(|k| k.entry == self.entry), target, fx);
                self.table.update(t.dt(), Self::layout().sectioned_table().h);
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                if let Some(i) = self.built.index_of(RowKey(to.elem)) {
                    self.table.sel = i as i32;
                    self.sel = Some(RowKey(to.elem));
                }
                // the keyboard belongs to the focused field
                let field = match to.elem {
                    EMAIL => Some(Field::Email),
                    PASSWORD => Some(Field::Password),
                    _ => None,
                };
                if self.editing.is_some() && self.editing != field {
                    self.keyboard(None, fx);
                }
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.activate(*elem, cx, fx);
                Handled::Yes
            }
            ScreenEvent::Input(input) => match &input.kind {
                InputKind::SystemKeyboard(up) => {
                    if !*up {
                        // the television put its panel away
                        self.editing = None;
                        self.edits += 1;
                    }
                    Handled::Yes
                }
                InputKind::Text(edit) if self.editing.is_some() => {
                    self.edit(edit);
                    fx.invalidate(Provenance::Input);
                    Handled::Yes
                }
                InputKind::Key { key, sym, edge, .. } if *edge != Edge::Up && self.editing.is_some() => {
                    match self.editing_key(*key, *sym, cx, fx) {
                        Some(()) => Handled::Yes,
                        None => Handled::No,
                    }
                }
                _ => Handled::No,
            },
            ScreenEvent::WillLeave(_) | ScreenEvent::Unmount | ScreenEvent::Cover | ScreenEvent::Suspend => {
                self.keyboard(None, fx);
                if matches!(ev, ScreenEvent::Unmount) {
                    fx.push(Fx::App(AppFx::Account(AccountCmd::CancelLink)));
                }
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

sn_ui::focusable_via_view!(SignInScreen, Bundle, view);

/// The narrative column's QR block: under the copy, above the bottom of the column.
fn qr_frame() -> Rect {
    let n = SignInScreen::layout().narrative;
    let h = n.h * 0.55;
    Rect::new(n.x, n.y + n.h - h, n.w, h)
}

impl Screen<Bundle> for SignInScreen {
    fn name(&self) -> &'static str {
        word::SIGN_IN
    }
    fn state(&self) -> &dyn LogicalState {
        self
    }
    fn crumb(&self, _cx: &Cx<'_, Bundle>) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(msg::app_nav_home()))
    }
    fn prepare(&mut self, _b: &mut sn_ui::frame::Budget, cx: &Cx<'_, Bundle>) {
        if let (Some((_, qr)), Link::Waiting { code, .. }) = (&mut self.qr, cx.views.account.link()) {
            let layout = QrLink::new(&code.code, LINK_ADDRESS).layout(qr_frame(), cx.measure);
            qr.prepare(layout.code);
        }
    }
    fn draw(&mut self, f: &mut DrawFrame<'_, '_, Bundle>) {
        sn_gfx::gfx::frame_clear(theme::CLEAR_RGB.0, theme::CLEAR_RGB.1, theme::CLEAR_RGB.2);
        {
            let mut v = self.view();
            Part::<Bundle>::draw(&mut v, f, Rect::FULL);
        }
        if let (Some((_, qr)), Link::Waiting { code, .. }) = (&self.qr, f.cx.views.account.link()) {
            let p = f.painter.alpha(f.page_alpha);
            let layout = QrLink::new(&code.code, LINK_ADDRESS).draw(p, qr_frame(), f.measure);
            qr.draw(p, layout.code);
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
    use crate::stores::account::AccountLanding;
    use sn_stremio::account::{ApiError, LinkCode};
    use sn_stremio::FetchError;

    fn labels(b: &Built) -> Vec<Vec<String>> {
        b.sections.iter().map(|s| s.rows.iter().map(|r| r.label.clone()).collect()).collect()
    }

    fn typed(s: &str) -> TextBuffer {
        TextBuffer::new(s.to_string(), s.len())
    }

    #[test]
    fn the_form_shows_each_state_of_an_attempt() {
        let mut store = AccountStore::signed_out(false);
        let b = build(&store, &typed(""), &typed(""), None);
        assert_eq!(b.sections[0].rows[0].value.as_deref(), Some(msg::app_signin_not_entered()));
        assert!(b.sections[0].rows[2].dim, "Sign in waits for both fields");
        store.command(AccountCmd::SignIn { email: "a@b.c".into(), password: Secret::new("pw") });
        let b = build(&store, &typed("a@b.c"), &typed("pw"), None);
        assert_eq!(labels(&b)[0][3], msg::app_signin_working());
        store.land(AccountLanding::LoggedIn(Err(ApiError::Api { code: 3, message: "Wrong passphrase".into() })), 0);
        let b = build(&store, &typed("a@b.c"), &typed("pw"), None);
        assert_eq!(labels(&b)[0][3], msg::app_signin_rejected("Wrong passphrase"), "the API's own words");
        assert!(b.index_of(RowKey(RETRY)).is_none(), "a refusal offers no retry");
        store.command(AccountCmd::SignIn { email: "a@b.c".into(), password: Secret::new("pw2") });
        store.land(AccountLanding::LoggedIn(Err(ApiError::Fetch(FetchError::Transport("timed out".into())))), 0);
        let b = build(&store, &typed("a@b.c"), &typed("pw2"), None);
        assert_eq!(labels(&b)[0][3], msg::app_signin_offline(msg::app_reason_timeout()));
        assert!(b.index_of(RowKey(RETRY)).is_some(), "a network failure offers a retry");
    }

    #[test]
    fn the_password_is_masked_and_the_caret_shows_while_typing() {
        let store = AccountStore::signed_out(false);
        let b = build(&store, &typed("a@b"), &TextBuffer::new("sécret".into(), "sé".len()), Some(Field::Password));
        assert_eq!(b.sections[0].rows[0].value.as_deref(), Some("a@b"));
        assert_eq!(b.sections[0].rows[1].value.as_deref(), Some("\u{2022}\u{2022}|\u{2022}\u{2022}\u{2022}\u{2022}"));
        let all = format!("{:?}", b.sections.iter().map(|s| s.rows.iter().map(|r| (&r.label, &r.value, &r.detail)).collect::<Vec<_>>()).collect::<Vec<_>>());
        assert!(!all.contains("cret"), "the password never reaches the table");
    }

    #[test]
    fn the_phone_section_follows_the_link() {
        let mut store = AccountStore::signed_out(false);
        store.command(AccountCmd::StartLink);
        assert_eq!(labels(&build(&store, &typed(""), &typed(""), None))[1], [msg::app_signin_link_creating()]);
        let code = LinkCode { code: "AB12".into(), link: "https://link.stremio.com/AB12".into(), qrcode: String::new() };
        store.land(AccountLanding::LinkCreated(Ok(code)), 0);
        assert_eq!(labels(&build(&store, &typed(""), &typed(""), None))[1], [msg::app_signin_link_waiting()]);
        store.command(AccountCmd::PollLink { now_ms: crate::stores::account::LINK_TTL_MS });
        let b = build(&store, &typed(""), &typed(""), None);
        assert_eq!(labels(&b)[1], [msg::app_signin_link_expired().to_string(), msg::app_signin_link_new().to_string()]);
    }

    #[test]
    fn every_app_string_fits_in_every_shipped_language() {
        let mut store = AccountStore::signed_out(false);
        store.command(AccountCmd::SignIn { email: "a@b.c".into(), password: Secret::new("pw") });
        store.land(AccountLanding::LoggedIn(Err(ApiError::Fetch(FetchError::Transport("timed out".into())))), 0);
        store.command(AccountCmd::StartLink);
        store.land(AccountLanding::LinkCreated(Err(ApiError::Fetch(FetchError::Status(503)))), 0);
        let w = RouteLayout::screen().sectioned_table().w;
        let mut fails = Vec::new();
        for lang in sn_platform::i18n::SHIPPED {
            let _g = sn_platform::i18n::language_on_this_thread_for_test(lang);
            for editing in [None, Some(Field::Email)] {
                let b = build(&store, &typed(""), &typed(""), editing);
                let mut t = TableView::new();
                t.open_sections(b.sections);
                fails.extend(t.app_fit_failures(w, &format!("signin {lang:?}")));
            }
        }
        sn_ui::table::assert_no_fit_failures(&fails);
    }
}
