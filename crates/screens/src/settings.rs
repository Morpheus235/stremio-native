//! **Settings** — the tab destination and the family of pages under it, PlxNative's Settings
//! idiom: a route header beside a `FormTable`, every row focused by identity, every drill-down a
//! push on the shared stack whose crumb names where BACK returns.
//!
//! - **The root** (a tab page, under the top bar): *Account* — who is signed in and *Sign out*
//!   (signing out is the top bar's account menu's, which asks first), or *Sign in*, then *Addons* (the Addons page, pushed: BACK returns to the row); *Playback* — skip
//!   intros, auto-play next episode, when Up Next appears, next episode, Dolby Vision profile 7;
//!   *Audio & subtitles* — the audio and subtitle language lists, the subtitle mode, forced
//!   fallback, subtitle size and position, text colour, background, outline colour, opacity and
//!   Style ASS subtitles; *Streams* —
//!   stream sorting; *Interface* — Stats for nerds and the key map; *About* — About StremioNative,
//!   Legal notices, Privacy, and the version line. Each section is a region on a card of its own
//!   (`TableView::grouped`).
//! - **A picker** ([`SettingsPage::Picker`]): one checked row per rung; OK sets it (the
//!   application persists it and publishes it to the player) and returns.
//! - **A language list** ([`SettingsPage::Languages`]): the chosen languages in order, then the
//!   common ones to add. OK on a language to add appends it; OK on a chosen one opens its popover
//!   (*Move up*, *Move down*, *Remove*, `ChoiceTarget::SettingsLanguage`). The stream order's page
//!   leads with *Same as audio*, the switch that makes it follow the audio list.
//! - **Legal notices** ([`SettingsPage::Legal`]): the third-party components, and the full texts
//!   of the GPL, the fonts' OFL and the copyright notice — the package's own files, embedded so
//!   reading them never touches the disk on the frame thread.
//! - **A document** ([`SettingsPage::Doc`]): scrolls inside on UP/DOWN and leaves at its ends.
//!
//! The builders ([`root_form`], [`picker_form`], [`languages_form`], [`legal_form`], [`body`]) are pure, so the fit
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

use crate::registry::{word, AppArg, AppFx, AppMsg, Bundle, ChoiceTarget, Doc, ListRow, MenuKind, SettingsCmd, SettingsPage};
use crate::stores::account::{AccountStore, SessionState};
use crate::stores::settings::{LangList, LangOp, Pref, SettingsStore};
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
    /// Opens the Addons page (pushed; BACK returns here).
    Addons,
    /// Stats for nerds, on or off: the read-out over every page (the device card before anything
    /// has played). OK toggles it.
    Stats,
    /// Skip intros: Off, Show button, Automatic (the picker).
    SkipIntros,
    /// Dolby Vision Profile 7: HDR10 or Try Dolby Vision (the picker).
    DvProfile7,
    /// Stream sorting: the languages a stream list puts first (the language list).
    StreamLanguages,
    /// Audio languages: the audio track a file starts on (the language list).
    AudioLanguages,
    /// Subtitle languages (the language list).
    SubtitleLanguages,
    /// Subtitles: Off, On, Forced only, Auto (the picker).
    SubtitleMode,
    /// Next episode: Same release, Always choose (the picker).
    NextEpisode,
    /// Forced fallback: Off, On (the picker).
    ForcedFallback,
    /// The text subtitles' colour, background, outline and opacity, and Style ASS subtitles (pickers).
    SubtitleColor,
    SubtitleBackground,
    SubtitleOutline,
    SubtitleOpacity,
    AssStyling,
    /// Auto-play next episode and when Up Next appears (pickers).
    AutoPlay,
    UpNextAt,
    /// The interface language (the picker; applies at the next launch).
    Language,
    /// Multichannel audio: Automatic, Dolby Digital, PCM, Stereo, Pro Logic II (the picker).
    MultichannelAudio,
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

/// The label of rung `r` of `pref` ([`Pref::rung_label`], which the player's menus read too).
pub fn rung_label(pref: Pref, r: u8) -> &'static str {
    pref.rung_label(r)
}

fn pref_label(pref: Pref) -> &'static str {
    pref.label()
}

fn pref_copy(pref: Pref) -> &'static str {
    match pref {
        Pref::SubtitleSize => msg::app_settings_subtitle_size_copy(),
        Pref::SubtitlePosition => msg::app_settings_subtitle_position_copy(),
        Pref::SkipIntros => msg::app_settings_skip_intros_copy(),
        Pref::DvProfile7 => msg::app_settings_dv_profile7_copy(),
        Pref::SubtitleMode => msg::app_settings_subtitle_mode_copy(),
        Pref::NextEpisode => msg::app_settings_next_episode_copy(),
        Pref::ForcedFallback => msg::app_settings_forced_fallback_copy(),
        Pref::SubtitleColor => msg::app_settings_subtitle_color_copy(),
        Pref::SubtitleBackground => msg::app_settings_subtitle_background_copy(),
        Pref::SubtitleOutline => msg::app_settings_subtitle_outline_copy(),
        Pref::SubtitleOpacity => msg::app_settings_subtitle_opacity_copy(),
        Pref::AssStyling => msg::app_settings_ass_styling_copy(),
        Pref::AutoPlay => msg::app_settings_auto_play_copy(),
        Pref::UpNextAt => msg::app_settings_up_next_at_copy(),
        Pref::Language => msg::app_settings_language_copy(),
        Pref::MultichannelAudio => msg::app_settings_multichannel_audio_copy(),
    }
}

/// A language list's title (its root row and its page).
pub fn languages_label(l: LangList) -> &'static str {
    match l {
        LangList::Audio => msg::app_settings_audio_languages(),
        LangList::Subtitles => msg::app_settings_subtitle_languages(),
        LangList::Streams => msg::app_settings_stream_languages(),
    }
}

fn languages_copy(l: LangList) -> &'static str {
    match l {
        LangList::Audio => msg::app_settings_audio_languages_copy(),
        LangList::Subtitles => msg::app_settings_subtitle_languages_copy(),
        LangList::Streams => msg::app_settings_stream_languages_copy(),
    }
}

/// A language's name for a list (its code is one `sn_base::tracks::LANGUAGES` names).
fn language_label(code: &str) -> String {
    sn_base::tracks::language_name(code).unwrap_or_else(|| code.to_string())
}

/// A language list's root value: *Same as audio*, the one language, or the first and how many
/// follow; with none chosen what that means (the file's default track, the app's language).
pub fn languages_value(settings: &SettingsStore, l: LangList) -> String {
    if l == LangList::Streams && settings.streams_follow_audio() {
        return msg::app_settings_stream_languages_same().to_string();
    }
    match settings.languages(l) {
        [] if l == LangList::Audio => msg::app_settings_audio_languages_none().to_string(),
        [] => msg::app_settings_subtitle_languages_none().to_string(),
        [one] => language_label(one),
        [first, rest @ ..] => msg::app_settings_languages_summary(&language_label(first), rest.len() as i64),
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
            // signing out is the top bar's account menu's (it asks first): no second path here
            if account.save_failed() { s.note(msg::app_addons_not_kept()) } else { s }
        }
    }
}

/// The root's form, from the settings and the session as they stand.
pub fn root_form(settings: &SettingsStore, account: &AccountStore, stats: bool) -> Form<RootRow, RootRow, SettingsPage> {
    let nav = |p: SettingsPage| RowKind::Nav(p);
    // a picker's row: its label and the rung it is set to
    let pick = |s: FormSection<RootRow, RootRow, SettingsPage>, id: RootRow, pref: Pref| {
        s.item(id, nav(SettingsPage::Picker(pref)), id, Row::new(pref_label(pref)).value(rung_label(pref, settings.get(pref))))
    };
    // a language list's row: its title and what it holds
    let langs = |s: FormSection<RootRow, RootRow, SettingsPage>, id: RootRow, l: LangList| {
        s.item(id, nav(SettingsPage::Languages(l)), id, Row::new(languages_label(l)).value(languages_value(settings, l)))
    };
    let playback = FormSection::new(msg::app_settings_playback());
    let playback = pick(playback, RootRow::SkipIntros, Pref::SkipIntros);
    let playback = pick(playback, RootRow::AutoPlay, Pref::AutoPlay);
    let playback = pick(playback, RootRow::UpNextAt, Pref::UpNextAt);
    let playback = pick(playback, RootRow::NextEpisode, Pref::NextEpisode);
    let playback = pick(playback, RootRow::DvProfile7, Pref::DvProfile7);
    let audio = FormSection::new(msg::app_settings_audio_subtitles());
    let audio = langs(audio, RootRow::AudioLanguages, LangList::Audio);
    let audio = pick(audio, RootRow::MultichannelAudio, Pref::MultichannelAudio);
    let audio = langs(audio, RootRow::SubtitleLanguages, LangList::Subtitles);
    let audio = pick(audio, RootRow::SubtitleMode, Pref::SubtitleMode);
    let audio = pick(audio, RootRow::ForcedFallback, Pref::ForcedFallback);
    let audio = pick(audio, RootRow::SubtitleSize, Pref::SubtitleSize);
    let audio = pick(audio, RootRow::SubtitlePosition, Pref::SubtitlePosition);
    let audio = pick(audio, RootRow::SubtitleColor, Pref::SubtitleColor);
    let audio = pick(audio, RootRow::SubtitleBackground, Pref::SubtitleBackground);
    let audio = pick(audio, RootRow::SubtitleOutline, Pref::SubtitleOutline);
    let audio = pick(audio, RootRow::SubtitleOpacity, Pref::SubtitleOpacity);
    let audio = pick(audio, RootRow::AssStyling, Pref::AssStyling);
    let streams = langs(FormSection::new(msg::app_settings_streams()), RootRow::StreamLanguages, LangList::Streams);
    Form::new()
        .section(account_section(account).item(
            RootRow::Addons,
            RowKind::Button,
            RootRow::Addons,
            Row::new(msg::app_addons_title()).chevron(true),
        ))
        .section(playback)
        .section(audio)
        .section(streams)
        .section(
            pick(FormSection::new(msg::app_settings_interface()), RootRow::Language, Pref::Language)
                .item(
                    RootRow::Stats,
                    RowKind::Button,
                    RootRow::Stats,
                    Row::new(msg::app_player_stats()).value(crate::player::tracks::stats_value(stats)),
                )
                .item(
                    RootRow::RemoteKeys,
                    nav(SettingsPage::Doc(Doc::Remote)),
                    RootRow::RemoteKeys,
                    Row::new(msg::app_settings_remote_keys()).detail(msg::app_settings_remote_keys_detail()),
                ),
        )
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
        let row = Row::new(rung_label(pref, r));
        // the rungs whose names do not say who they are for
        use crate::stores::settings::{MULTI_DPLII, MULTI_STEREO};
        let row = match (pref, r) {
            (Pref::MultichannelAudio, MULTI_STEREO) => row.detail(msg::app_settings_multichannel_stereo_hint()),
            (Pref::MultichannelAudio, MULTI_DPLII) => row.detail(msg::app_settings_multichannel_dplii_hint()),
            _ => row,
        };
        s = s.choice(Rung(r), Rung(r), row, |id| id.0 == current);
    }
    Form::new().section(s)
}

/// A language list page's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LangRow {
    /// The stream order's *Same as audio* switch.
    SameAsAudio,
    /// The `n`-th chosen language, by its [`sn_base::tracks::LANGUAGES`] index (the key stays with
    /// the language when it moves).
    Chosen { at: usize, lang: usize },
    /// A language to add, by its [`sn_base::tracks::LANGUAGES`] index.
    Add(usize),
}

impl FormId for LangRow {
    fn key(&self) -> RowKey {
        RowKey(match self {
            LangRow::SameAsAudio => 1,
            LangRow::Chosen { lang, .. } => 0x10 + *lang as u32,
            LangRow::Add(lang) => 0x80 + *lang as u32,
        })
    }
}

/// A language list's page: on the stream order the *Same as audio* switch first; the chosen
/// languages in order (a note while none is; the audio list's while the order follows it, which an
/// edit then copies); then every common language not chosen, to add.
pub fn languages_form(settings: &SettingsStore, l: LangList) -> Form<LangRow, LangRow, Infallible> {
    let mut form = Form::new();
    if l == LangList::Streams {
        let follow = settings.streams_follow_audio();
        form = form.section(FormSection::new("").choice(
            LangRow::SameAsAudio,
            LangRow::SameAsAudio,
            Row::new(msg::app_settings_stream_languages_same()),
            |_| follow,
        ));
    }
    let chosen = settings.languages(l);
    let mut s = FormSection::new(msg::app_settings_languages_chosen());
    if chosen.is_empty() {
        s = s.note(msg::app_settings_languages_none());
    }
    for (at, code) in chosen.iter().enumerate() {
        let Some(lang) = sn_base::tracks::language_index(code) else { continue };
        let id = LangRow::Chosen { at, lang };
        s = s.item(id.clone(), RowKind::Button, id, Row::new(language_label(code)).value((at + 1).to_string()));
    }
    let mut add = FormSection::new(msg::app_settings_languages_add());
    for code in sn_base::tracks::COMMON.iter().filter(|c| !chosen.iter().any(|x| x == *c)) {
        let Some(lang) = sn_base::tracks::language_index(code) else { continue };
        add = add.item(LangRow::Add(lang), RowKind::Button, LangRow::Add(lang), Row::new(language_label(code)));
    }
    form.section(s).section(add)
}

/// A chosen language's popover: its rows, in [`LANGUAGE_OPS`] order.
pub fn language_menu(code: &str) -> MenuKind {
    let row = |label: &str| ListRow { label: label.to_string(), value: String::new(), server_value: false, opens: false, switch: None };
    MenuKind::List {
        title: language_label(code),
        rows: vec![row(msg::app_settings_language_up()), row(msg::app_settings_language_down()), row(msg::app_settings_language_remove())],
        target: ChoiceTarget::SettingsLanguage,
    }
}

/// What row `i` of [`language_menu`] does to the `at`-th chosen language.
pub fn language_op(i: usize, at: usize) -> Option<LangOp> {
    match i {
        0 => Some(LangOp::Up(at)),
        1 => Some(LangOp::Down(at)),
        2 => Some(LangOp::Remove(at)),
        _ => None,
    }
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
            msg::app_privacy_skip().to_string(),
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
        line(msg::app_remote_key_yellow(), msg::app_remote_see_all()),
        line(msg::app_remote_key_blue(), msg::app_tab_search()),
        line(msg::app_remote_key_ch_up(), msg::app_remote_prev_tab()),
        line(msg::app_remote_key_ch_down(), msg::app_remote_next_tab()),
        String::new(),
        msg::app_remote_playback_head().to_uppercase(),
        line(msg::app_remote_key_play(), msg::app_remote_pause()),
        line(msg::app_remote_key_stop(), msg::app_remote_stop()),
        line(msg::app_remote_key_left_right(), msg::app_remote_seek_step()),
        line(msg::app_remote_key_rew_ff(), msg::app_remote_speed_scrub()),
        line(msg::app_remote_key_digits(), msg::app_remote_jump_percent()),
        line(msg::app_remote_key_ch(), msg::app_remote_jump_ten()),
        line(msg::app_remote_key_red(), msg::app_remote_audio_menu()),
        line(msg::app_remote_key_green(), msg::app_remote_subtitles_toggle()),
        line(msg::app_remote_key_yellow(), msg::app_remote_episodes()),
        line(msg::app_remote_key_blue(), msg::app_remote_show_info()),
    ]
    .join("\n")
}

enum Mode {
    Root(FormTable<RootRow, RootRow, SettingsPage>),
    Picker(Pref, FormTable<Rung, Rung, Infallible>),
    /// A language list, and the chosen language whose popover is open (its place in the list).
    Languages(LangList, FormTable<LangRow, LangRow, Infallible>, Option<usize>),
    Legal(FormTable<LegalRow, LegalRow, SettingsPage>),
    Doc { doc: Doc, reader: DocumentReader, body: String, copy: String },
}

pub struct SettingsScreen {
    entry: EntryId,
    mode: Mode,
    /// The settings and account generations the root and the picker were built from.
    built: Option<(u32, u32, bool)>,
}

impl SettingsScreen {
    /// The tab destination.
    pub fn root(entry: EntryId) -> Self {
        SettingsScreen { entry, mode: Mode::Root(grouped_table()), built: None }
    }

    /// A page of the family.
    pub fn page(entry: EntryId, page: SettingsPage) -> Self {
        let mode = match page {
            SettingsPage::Picker(p) => Mode::Picker(p, grouped_table()),
            SettingsPage::Languages(l) => Mode::Languages(l, grouped_table(), None),
            SettingsPage::Legal => {
                let mut f = grouped_table();
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
            Mode::Languages(_, f, _) => Some(&f.table),
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
            Mode::Languages(l, _, _) => languages_label(*l),
            Mode::Legal(_) => msg::app_settings_legal(),
            Mode::Doc { doc, .. } => doc_title(*doc),
        }
    }

    fn copy(&self) -> &str {
        match &self.mode {
            Mode::Root(_) => msg::app_settings_copy(),
            Mode::Picker(p, _) => pref_copy(*p),
            Mode::Languages(l, _, _) => languages_copy(*l),
            Mode::Legal(_) => msg::app_settings_legal_copy(),
            Mode::Doc { copy, .. } => copy,
        }
    }

    fn doc_frame(&self) -> Rect {
        self.layout().document(self.crumb().is_some())
    }

    /// Rebuild the root / the picker when the settings or the session moved.
    fn sync(&mut self, store: &SettingsStore, account: &AccountStore, stats: bool) {
        let gen = (store.gen(), account.gen(), stats);
        if self.built == Some(gen) {
            return;
        }
        let fresh = self.built.is_none();
        self.built = Some(gen);
        match &mut self.mode {
            Mode::Root(f) => {
                if fresh {
                    f.set(root_form(store, account, stats), None);
                } else {
                    f.refresh(root_form(store, account, stats));
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
            Mode::Languages(l, f, _) => {
                if fresh {
                    f.set(languages_form(store, *l), None);
                } else {
                    f.refresh(languages_form(store, *l));
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
            Mode::Languages(_, f, _) => TableScreen::new(header, &f.table, GROUP, self.entry).keyed(f),
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
            Mode::Languages(_, f, _) => focus_row(f, key),
            Mode::Legal(f) => focus_row(f, key),
            Mode::Doc { .. } => {}
        }
    }

    fn activate(&mut self, elem: u32, cx: &Cx<'_, Bundle>, fx: &mut Effects<'_, Bundle>) {
        let key = RowKey(elem);
        if let Mode::Languages(l, f, _) = &self.mode {
            let (l, act) = (*l, f.index_of_key(key).and_then(|i| f.activate(i)));
            match act {
                Some(Activation::Action(LangRow::SameAsAudio)) => {
                    let follow = cx.views.settings.streams_follow_audio();
                    fx.push(Fx::App(AppFx::Settings(SettingsCmd::Languages(l, LangOp::SameAsAudio(!follow)))));
                }
                Some(Activation::Action(LangRow::Add(lang))) => {
                    let code = sn_base::tracks::LANGUAGES[lang].0;
                    fx.push(Fx::App(AppFx::Settings(SettingsCmd::Languages(l, LangOp::Add(code)))));
                }
                Some(Activation::Action(LangRow::Chosen { at, lang })) => {
                    // the popover hangs beside the row's right end (`crate::menu::panel_at`)
                    let row = self.table_view().and_then(|v| Focusable::<Bundle>::place(&v, &elem, cx, At::SpringTarget));
                    let at_rect = row.map_or_else(|| self.layout().sectioned_table(), |p| p.rect);
                    let anchor = Rect::new(at_rect.x + at_rect.w - 1.0, at_rect.y, 1.0, at_rect.h.min(theme::size::BODY as f32 * 2.0));
                    if let Mode::Languages(_, _, editing) = &mut self.mode {
                        *editing = Some(at);
                    }
                    crate::dropdown::open(self.entry, cx, fx, anchor, language_menu(sn_base::tracks::LANGUAGES[lang].0));
                }
                Some(Activation::Push(never)) => match never {},
                None => {}
            }
            fx.invalidate(Provenance::Input);
            return;
        }
        match &self.mode {
            Mode::Root(f) => match f.index_of_key(key).and_then(|i| f.activate(i)) {
                Some(Activation::Push(page)) => fx.push(Fx::Nav(NavOp::Push(AppArg::SettingsPage(page)))),
                Some(Activation::Action(RootRow::SignIn)) => fx.push(Fx::Nav(NavOp::Push(AppArg::SignIn))),
                Some(Activation::Action(RootRow::Addons)) => fx.push(Fx::Nav(NavOp::Push(AppArg::Addons))),
                // who is signed in: the account menu (Sign out, asked first), beside the row
                Some(Activation::Action(RootRow::Account)) => {
                    let row = self.table_view().and_then(|v| Focusable::<Bundle>::place(&v, &elem, cx, At::SpringTarget));
                    let r = row.map_or_else(|| self.layout().sectioned_table(), |p| p.rect);
                    let anchor = Rect::new(r.x + r.w - 1.0, r.y, 1.0, r.h);
                    crate::tabs::open_account_menu_at(self.entry, FocusKey { entry: self.entry, elem }, anchor, fx);
                }
                Some(Activation::Action(RootRow::Stats)) => {
                    fx.push(Fx::App(AppFx::Player(crate::registry::PlayerCmd::ToggleStats)));
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
            Mode::Languages(..) | Mode::Doc { .. } => {}
        }
    }

    /// A chosen language's popover answered with row `i`.
    fn chose_language(&mut self, i: usize, fx: &mut Effects<'_, Bundle>) {
        let Mode::Languages(l, _, editing) = &mut self.mode else { return };
        if let Some(op) = editing.take().and_then(|at| language_op(i, at)) {
            fx.push(Fx::App(AppFx::Settings(SettingsCmd::Languages(*l, op))));
        }
    }

    fn tick(&mut self, dt: f32) {
        let h = self.layout().sectioned_table().h;
        match &mut self.mode {
            Mode::Root(f) => f.table.update(dt, h),
            Mode::Picker(_, f) => f.table.update(dt, h),
            Mode::Languages(_, f, _) => f.table.update(dt, h),
            Mode::Legal(f) => f.table.update(dt, h),
            Mode::Doc { reader, .. } => reader.update(dt),
        }
    }
}

/// A settings page's table: its sections are regions, each on a card of its own.
fn grouped_table<Id: PartialEq + Clone, A: Clone, D: Clone>() -> FormTable<Id, A, D> {
    let mut f = FormTable::new(KEY_BAND);
    f.table.grouped = true;
    f
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
            Mode::Languages(l, f, _) => {
                c.u32(4).u32(*l as u32).u32(f.table.sel as u32);
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
                self.sync(cx.views.settings, cx.views.account, cx.views.playback.stats);
                // a picker opens on the rung in force, not on its first row
                if let Mode::Picker(p, _) = &self.mode {
                    let key = Rung(cx.views.settings.get(*p)).key();
                    crate::registry::reseat(sn_ui::screen::FocusTarget::Elem(FocusKey { entry: self.entry, elem: key.0 }), fx);
                }
                Handled::Yes
            }
            ScreenEvent::StoreChanged(ord, _) if *ord == StoreId::Settings.ord() || *ord == StoreId::Account.ord() => {
                self.sync(cx.views.settings, cx.views.account, cx.views.playback.stats);
                fx.invalidate(Provenance::Landing(fx.from()));
                Handled::Yes
            }
            ScreenEvent::Tick(t) => {
                self.sync(cx.views.settings, cx.views.account, cx.views.playback.stats);
                self.tick(t.dt());
                Handled::Yes
            }
            ScreenEvent::FocusMoved { to, .. } => {
                self.note_focus(to.elem);
                fx.invalidate(Provenance::Input);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) if crate::tabs::on_strip(*elem) => {
                crate::tabs::on_pill(*elem, Tab::Settings, Some(GROUP), cx, fx);
                Handled::Yes
            }
            ScreenEvent::Activate(elem) => {
                self.activate(*elem, cx, fx);
                Handled::Yes
            }
            ScreenEvent::App(AppMsg::Chose(ChoiceTarget::SettingsLanguage, i)) => {
                self.chose_language(*i, fx);
                Handled::Yes
            }
            ScreenEvent::PressCommit(_) => {
                if let Some(k) = cx.focus.current.filter(|k| k.entry == self.entry) {
                    if crate::tabs::on_strip(k.elem) {
                        crate::tabs::on_pill(k.elem, Tab::Settings, Some(GROUP), cx, fx);
                    } else {
                        self.activate(k.elem, cx, fx);
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
                    for (who, account, stats) in accounts().into_iter().chain(accounts()).zip([false, false, false, true, true, true]).map(|((w, a), s)| (w, a, s)) {
                        // the Skip intros value moves with its own rung, not with the others'
                        let skip = (size + position) % Pref::SkipIntros.rungs();
                        let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
                        // every shape of the language rows' trailing values: none, one, several,
                        // following audio, and the widest names
                        let lists = language_cases();
                        let (audio, subtitles, streams) = &lists[usize::from(size + position) % lists.len()];
                        f.set(root_form(&SettingsStore::with(size, position, skip).with_dv_profile7(skip % 2)
                            .with_languages(audio, subtitles, streams.as_deref())
                            .seeded(Pref::SubtitleMode, (size + position) % Pref::SubtitleMode.rungs())
                            .seeded(Pref::ForcedFallback, (size + position) % Pref::ForcedFallback.rungs())
                            .seeded(Pref::NextEpisode, (size + position) % Pref::NextEpisode.rungs())
                            .seeded(Pref::SubtitleColor, (size + position) % Pref::SubtitleColor.rungs())
                            .seeded(Pref::SubtitleBackground, (size + position) % Pref::SubtitleBackground.rungs())
                            .seeded(Pref::SubtitleOutline, (size + position) % Pref::SubtitleOutline.rungs())
                            .seeded(Pref::SubtitleOpacity, (size + position) % Pref::SubtitleOpacity.rungs())
                            .seeded(Pref::AssStyling, (size + position) % Pref::AssStyling.rungs())
                            .seeded(Pref::AutoPlay, (size + position) % Pref::AutoPlay.rungs())
                            .seeded(Pref::UpNextAt, (size + position) % Pref::UpNextAt.rungs()), &account, stats), None);
                        let tag = format!("{tag} root {who} size={size} position={position} skip={skip} stats={stats}");
                        fit(&tag, &f.table, RouteLayout::below_top_bar(), &mut out);
                    }
                }
            }
            for pref in Pref::ALL {
                let mut f = FormTable::<Rung, Rung, Infallible>::new(KEY_BAND);
                f.set(picker_form(pref, pref.default_rung()), None);
                fit(&format!("{tag} picker {pref:?}"), &f.table, RouteLayout::screen(), &mut out);
            }
            // every language list page, empty, following and full; and a chosen language's popover
            for (audio, subtitles, streams) in language_cases() {
                let store = SettingsStore::default().with_languages(&audio, &subtitles, streams.as_deref());
                for l in [LangList::Audio, LangList::Subtitles, LangList::Streams] {
                    let mut f = FormTable::<LangRow, LangRow, Infallible>::new(KEY_BAND);
                    f.set(languages_form(&store, l), None);
                    fit(&format!("{tag} languages {l:?}"), &f.table, RouteLayout::screen(), &mut out);
                }
            }
            let mut f = FormTable::<crate::menu::MenuRow, crate::menu::MenuRow, Infallible>::new(0x1000);
            f.table.compact = true;
            f.set(crate::menu::build(&language_menu("EN"), Default::default(), crate::stores::library::AddState::Absent), None);
            out.extend(f.table.app_fit_failures_hugged(&format!("{tag} language menu")));
            let mut f = FormTable::<LegalRow, LegalRow, SettingsPage>::new(KEY_BAND);
            f.set(legal_form(), None);
            fit(&format!("{tag} legal"), &f.table, RouteLayout::screen(), &mut out);
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    /// The root is grouped into regions, each on its own card: Account, Playback, Audio &
    /// subtitles, Streams, Interface, About — every row in the region it belongs to, and every
    /// region's header (app text, drawn in caps) fitting its column in every shipped language.
    #[test]
    fn the_root_is_grouped_into_regions_whose_headers_fit_in_every_shipped_language() {
        let id = |r: RootRow| sn_ui::form::FormId::key(&r).0;
        let mut out = Vec::new();
        for language in SHIPPED {
            let _g = language_on_this_thread_for_test(language);
            let mut f: FormTable<RootRow, RootRow, SettingsPage> = grouped_table();
            f.set(root_form(&SettingsStore::default(), &signed_in("a@example.com"), false), None);
            assert!(f.table.grouped, "each region is a card");
            let headers: Vec<String> = f.table.sections.iter().map(|s| s.header.clone()).collect();
            assert_eq!(
                headers,
                [
                    msg::app_addons_account(),
                    msg::app_settings_playback(),
                    msg::app_settings_audio_subtitles(),
                    msg::app_settings_streams(),
                    msg::app_settings_interface(),
                    msg::app_settings_about_section(),
                ]
            );
            for issue in f.table.fit_report(RouteLayout::below_top_bar().sectioned_table().w, &sn_base::fontcov::advances::ShippedMeasure, sn_ui::fit::HEADROOM) {
                if issue.role == sn_ui::table::FitRole::Header {
                    out.push(format!("{}: {}", language.tag(), issue.describe()));
                }
            }
            if language.tag() == "en" {
                // the rows of each region, in order (notes are not rows)
                let keys: Vec<Vec<u32>> = {
                    let mut i = 0;
                    f.table.sections.iter().map(|s| s.rows.iter().filter_map(|r| { let k = (!r.sep).then(|| f.key_at(i).map(|k| k.0)).flatten(); i += 1; k }).collect()).collect()
                };
                use RootRow::*;
                let want: Vec<Vec<u32>> = [
                    vec![Account, Addons],
                    vec![SkipIntros, AutoPlay, UpNextAt, NextEpisode, DvProfile7],
                    vec![
                        AudioLanguages,
                        MultichannelAudio,
                        SubtitleLanguages,
                        SubtitleMode,
                        ForcedFallback,
                        SubtitleSize,
                        SubtitlePosition,
                        SubtitleColor,
                        SubtitleBackground,
                        SubtitleOutline,
                        SubtitleOpacity,
                        AssStyling,
                    ],
                    vec![StreamLanguages],
                    vec![Language, Stats, RemoteKeys],
                    vec![About, Legal, Privacy],
                ]
                .into_iter()
                .map(|v| v.into_iter().map(id).collect())
                .collect();
                assert_eq!(keys, want);
            }
        }
        sn_ui::table::assert_no_fit_failures(&out);
    }

    /// The language list shapes the fit test walks: `(audio, subtitles, streams)`.
    #[allow(clippy::type_complexity)]
    fn language_cases() -> Vec<(Vec<String>, Vec<String>, Option<Vec<String>>)> {
        let c = |l: &[&str]| l.iter().map(|c| (*c).to_string()).collect::<Vec<_>>();
        let all = c(&sn_base::tracks::COMMON);
        vec![
            (vec![], vec![], None),
            (c(&["DE"]), c(&["FR"]), Some(c(&["IT"]))),
            (c(&["DE", "EN"]), c(&["RU", "EN", "UK"]), None),
            // the longest names lead: Bahasa Indonesia, Українська, Nederlands
            (c(&["ID", "EN"]), c(&["UK", "DE"]), Some(c(&["NL", "SV", "NO"]))),
            (all.clone(), all.clone(), Some(vec![])),
        ]
    }

    /// The language list page: OK on a language to add appends it, OK on a chosen one opens its
    /// popover, and the popover's rows move it and remove it; the stream order's switch follows.
    #[test]
    fn a_language_page_adds_and_its_popover_moves_and_removes() {
        let store = SettingsStore::default().with_languages(&["DE".into(), "EN".into()], &[], None);
        let form = |store: &SettingsStore, l| {
            let mut f = FormTable::<LangRow, LangRow, Infallible>::new(KEY_BAND);
            f.set(languages_form(store, l), None);
            f
        };
        let f = form(&store, LangList::Audio);
        let ids: Vec<LangRow> = (0..f.table.n_rows() as usize).filter_map(|i| f.id_at(i).cloned()).collect();
        let de = sn_base::tracks::language_index("de").unwrap();
        let en = sn_base::tracks::language_index("en").unwrap();
        let fr = sn_base::tracks::language_index("fr").unwrap();
        assert_eq!(&ids[..3], [LangRow::Chosen { at: 0, lang: de }, LangRow::Chosen { at: 1, lang: en }, LangRow::Add(fr)], "chosen in order, then the rest");
        assert!(!ids.contains(&LangRow::Add(de)), "a chosen language is not offered again");
        assert_eq!(ids.len(), 2 + sn_base::tracks::COMMON.len() - 2);
        assert!(!ids.contains(&LangRow::SameAsAudio), "only the stream order follows another list");
        assert_eq!(form(&store, LangList::Streams).id_at(0), Some(&LangRow::SameAsAudio));
        assert_eq!(LangRow::Chosen { at: 0, lang: de }.key(), LangRow::Chosen { at: 1, lang: de }.key(), "a row's key stays with its language");
        assert_eq!(language_op(0, 1), Some(LangOp::Up(1)));
        assert_eq!(language_op(1, 1), Some(LangOp::Down(1)));
        assert_eq!(language_op(2, 0), Some(LangOp::Remove(0)));
        assert_eq!(language_op(3, 0), None);
        let _g = language_on_this_thread_for_test(sn_platform::i18n::Preference::En);
        assert_eq!(languages_value(&store, LangList::Audio), "Deutsch +1");
        assert_eq!(languages_value(&store, LangList::Streams), msg::app_settings_stream_languages_same());
        assert_eq!(languages_value(&store, LangList::Subtitles), msg::app_settings_subtitle_languages_none());
        assert_eq!(languages_value(&SettingsStore::default(), LangList::Audio), msg::app_settings_audio_languages_none());
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
        f.set(root_form(&SettingsStore::default(), &AccountStore::signed_out(true), false), None);
        let pushed: Vec<SettingsPage> = (0..f.table.n_rows() as usize)
            .filter_map(|i| match f.activate(i) {
                Some(Activation::Push(p)) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(
            pushed,
            vec![
                SettingsPage::Picker(Pref::SkipIntros),
                SettingsPage::Picker(Pref::AutoPlay),
                SettingsPage::Picker(Pref::UpNextAt),
                SettingsPage::Picker(Pref::NextEpisode),
                SettingsPage::Picker(Pref::DvProfile7),
                SettingsPage::Languages(LangList::Audio),
                SettingsPage::Picker(Pref::MultichannelAudio),
                SettingsPage::Languages(LangList::Subtitles),
                SettingsPage::Picker(Pref::SubtitleMode),
                SettingsPage::Picker(Pref::ForcedFallback),
                SettingsPage::Picker(Pref::SubtitleSize),
                SettingsPage::Picker(Pref::SubtitlePosition),
                SettingsPage::Picker(Pref::SubtitleColor),
                SettingsPage::Picker(Pref::SubtitleBackground),
                SettingsPage::Picker(Pref::SubtitleOutline),
                SettingsPage::Picker(Pref::SubtitleOpacity),
                SettingsPage::Picker(Pref::AssStyling),
                SettingsPage::Languages(LangList::Streams),
                SettingsPage::Picker(Pref::Language),
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
            f.set(root_form(&SettingsStore::default(), account, false), None);
            (0..f.table.n_rows() as usize)
                .filter_map(|i| match f.activate(i) {
                    Some(Activation::Action(a)) => Some(a),
                    _ => None,
                })
                .collect()
        };
        // the account section's actions lead, then Addons (later sections' rows follow)
        assert!(actions(&AccountStore::signed_out(false)).starts_with(&[RootRow::SignIn, RootRow::Addons]));
        assert!(actions(&signed_in("a@b.c")).starts_with(&[RootRow::Account, RootRow::Addons]), "no Sign out: the account menu owns it");
        assert!(actions(&AccountStore::new()).starts_with(&[RootRow::Addons]), "only Addons while the session is read");
        let mut f = FormTable::<RootRow, RootRow, SettingsPage>::new(KEY_BAND);
        f.set(root_form(&SettingsStore::default(), &signed_in("a@b.c"), false), None);
        let rows: Vec<&sn_ui::table::Row> = f.table.sections.iter().flat_map(|s| s.rows.iter()).collect();
        assert!(rows.iter().any(|r| r.value.as_deref() == Some("a@b.c")), "the account is shown");
    }
}
