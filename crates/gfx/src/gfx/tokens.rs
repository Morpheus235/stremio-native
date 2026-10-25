//! The design tokens the renderer itself draws with: the 8-bit colour constructor and the two palette
//! stops `gfx` paints with, the card shading constants its image shader mirrors, and the type-size
//! ladder `text` warms its faces from.
//!
//! They lived in `ui/theme.rs` and moved here (module-layers step L5) because `gfx` and `text` read
//! them and the `gfx` layer may not name `ui`. `ui::theme` imports `rgb8` and the two stops for its
//! private palette and re-exports everything else (`theme::CLEAR_RGB`, `theme::size::BODY`,
//! `theme::with_a`, `theme::CARD_GLOW_A`, …), so every screen still names the token in the place
//! `ui/CLAUDE.md` tells it to look: `theme.rs` stays the one palette, and a token's VALUE is written
//! down once, here.
//!
//! `tools/font-hint-audit.py` parses `pub mod size` out of THIS file.
// The same blanket `ui/theme.rs` carries: a token is added with its role and some rungs/constants
// are read only through the `ui::theme` re-export (or only by a test), which is not dead code.
#![allow(dead_code)]

/// An 8-bit sRGB code as an opaque token value. `rgb8(0x2c, 0x2c, 0x2e)` is `#2c2c2e`.
///
/// Every palette stop is an EXACT 8-bit code, which is load-bearing rather than tidy: the panel is
/// plain 888 with no sRGB framebuffer anywhere in the tree, so a token value IS an sRGB code — and a
/// FRACTIONAL one hands `GL_DITHER` (on by default in GLES2) a half-code to alternate on across a
/// large flat fill, which banded the app ground visibly before it was snapped. So write the code and
/// let this do the division; never a decimal guess.
pub const fn rgb8(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

// Neutral — achromatic. These two stops are the ones the renderer paints with directly; the rest of
// the palette is `ui::theme`'s private primitive layer, which imports these so a code is still
// written down in exactly one place.
/// The app ground (`theme::SURFACE_APP`'s stop): Stremio's purple-navy `#1a173e`, stremio-web's
/// `--secondary-background-color: rgba(26, 23, 62, 1)` (`src/App/styles.less`). The lighter end of
/// Stremio's `41deg` page gradient rather than its near-black `#0c0b11` end, so a focused card's
/// drop shadow still has a ground to darken. (Was Apple TV's shelf gray `#2c2c2e`; the name stays
/// so the stop keeps its one home.)
pub const NEUTRAL_500: [f32; 4] = PALETTE.ground;

/// The **hue-carrying stops**: the only ones a colour theme swaps. The white/black alpha ramps,
/// the semantic amber/red/green and the caption greys are shared by every theme, so a theme is a
/// ground tint plus an accent, not a whole palette (the nuvio TV client reached the same verdict:
/// its greys are tuned against one dark ground).
#[derive(Clone, Copy)]
pub struct Palette {
    /// The app ground (`NEUTRAL_500`).
    pub ground: [f32; 4],
    /// The panel stops, both darker than the ground: `NEUTRAL_650`, `NEUTRAL_750`.
    pub panel: [[f32; 4]; 2],
    /// The accent and its one-step-deeper focus pill: `VIOLET_500`, `VIOLET_550`.
    pub accent: [[f32; 4]; 2],
    /// Label ink ON the accent: white on a deep accent, near-black on a light one.
    pub accent_ink: [f32; 4],
    /// The pre-content wash's corners: `ATMOS_NIGHT`, `_DUSK`, `_VIOLET`, `_INDIGO`.
    pub atmos: [[f32; 4]; 4],
    /// Text inks, lightest first: `COOL_0/50/150/200/300/400`.
    pub text: [[f32; 4]; 6],
}

const TEXT_ON_DARK: [[f32; 4]; 6] = [
    rgb8(0xf7, 0xfa, 0xfc),
    rgb8(0xeb, 0xf0, 0xf7),
    rgb8(0xdb, 0xe0, 0xeb),
    rgb8(0xcd, 0xd3, 0xdd),
    rgb8(0xb8, 0xbf, 0xcc),
    rgb8(0x94, 0x99, 0xa3),
];
const INK_WHITE: [f32; 4] = rgb8(0xff, 0xff, 0xff);
const INK_DARK: [f32; 4] = rgb8(0x12, 0x13, 0x16);

/// Every theme by name, the shipped one first. Accents other than Stremio's are nuvio's TV-tuned
/// values; each ground is the Stremio ladder (ground, darker modal, darkest) re-hued.
pub const PALETTES: &[(&str, Palette)] = &[
    ("stremio", Palette {
        ground: rgb8(0x1a, 0x17, 0x3e),
        panel: [rgb8(0x0f, 0x0d, 0x20), rgb8(0x0c, 0x0b, 0x11)],
        accent: [rgb8(0x7b, 0x5b, 0xf5), rgb8(0x6e, 0x4e, 0xe8)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0x0c, 0x0b, 0x11), rgb8(0x26, 0x20, 0x5e), rgb8(0x2e, 0x28, 0x73), rgb8(0x0f, 0x0d, 0x20)],
        text: TEXT_ON_DARK,
    }),
    ("graphite", Palette {
        ground: rgb8(0x1c, 0x1c, 0x1e),
        panel: [rgb8(0x11, 0x11, 0x13), rgb8(0x0b, 0x0b, 0x0c)],
        accent: [rgb8(0xf4, 0xf2, 0xee), rgb8(0xdc, 0xda, 0xd5)],
        accent_ink: INK_DARK,
        atmos: [rgb8(0x0b, 0x0b, 0x0c), rgb8(0x2a, 0x2a, 0x2d), rgb8(0x33, 0x33, 0x36), rgb8(0x11, 0x11, 0x13)],
        text: TEXT_ON_DARK,
    }),
    ("ocean", Palette {
        ground: rgb8(0x12, 0x23, 0x3a),
        panel: [rgb8(0x0b, 0x16, 0x26), rgb8(0x08, 0x0e, 0x17)],
        accent: [rgb8(0x12, 0x76, 0xce), rgb8(0x0f, 0x68, 0xb8)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0x08, 0x0e, 0x17), rgb8(0x17, 0x35, 0x56), rgb8(0x1b, 0x41, 0x70), rgb8(0x0b, 0x16, 0x26)],
        text: TEXT_ON_DARK,
    }),
    ("crimson", Palette {
        ground: rgb8(0x2e, 0x14, 0x18),
        panel: [rgb8(0x1c, 0x0c, 0x0f), rgb8(0x12, 0x08, 0x09)],
        accent: [rgb8(0xd5, 0x3b, 0x45), rgb8(0xc0, 0x31, 0x3b)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0x12, 0x08, 0x09), rgb8(0x4a, 0x1d, 0x24), rgb8(0x5c, 0x20, 0x28), rgb8(0x1c, 0x0c, 0x0f)],
        text: TEXT_ON_DARK,
    }),
    ("emerald", Palette {
        ground: rgb8(0x10, 0x29, 0x1d),
        panel: [rgb8(0x09, 0x1a, 0x12), rgb8(0x06, 0x10, 0x0b)],
        accent: [rgb8(0x21, 0x86, 0x49), rgb8(0x1b, 0x78, 0x40)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0x06, 0x10, 0x0b), rgb8(0x1a, 0x44, 0x30), rgb8(0x1d, 0x55, 0x38), rgb8(0x09, 0x1a, 0x12)],
        text: TEXT_ON_DARK,
    }),
    ("gold", Palette {
        ground: rgb8(0x28, 0x22, 0x0f),
        panel: [rgb8(0x18, 0x14, 0x0a), rgb8(0x0f, 0x0d, 0x07)],
        accent: [rgb8(0xf2, 0xcd, 0x64), rgb8(0xe0, 0xbb, 0x52)],
        accent_ink: INK_DARK,
        atmos: [rgb8(0x0f, 0x0d, 0x07), rgb8(0x41, 0x38, 0x1a), rgb8(0x52, 0x46, 0x1c), rgb8(0x18, 0x14, 0x0a)],
        text: TEXT_ON_DARK,
    }),
    // True black for OLED: the ground's pixels switch off. Nothing is darker than black, so the
    // panels lift slightly above it instead, and the pre-content wash stays black.
    ("oled", Palette {
        ground: rgb8(0x00, 0x00, 0x00),
        panel: [rgb8(0x0e, 0x0e, 0x11), rgb8(0x08, 0x08, 0x0a)],
        accent: [rgb8(0x7b, 0x5b, 0xf5), rgb8(0x6e, 0x4e, 0xe8)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0x00, 0x00, 0x00), rgb8(0x06, 0x05, 0x0c), rgb8(0x0a, 0x08, 0x16), rgb8(0x00, 0x00, 0x00)],
        text: TEXT_ON_DARK,
    }),
    // ponytail: a light ground and dark text only. Every `with_a(WHITE, a)` overlay, scrim and
    // shader glow still assumes a dark ground; this entry shows what a real light mode must redo.
    ("light", Palette {
        ground: rgb8(0xf2, 0xf2, 0xf6),
        panel: [rgb8(0xff, 0xff, 0xff), rgb8(0xe6, 0xe6, 0xee)],
        accent: [rgb8(0x7b, 0x5b, 0xf5), rgb8(0x6e, 0x4e, 0xe8)],
        accent_ink: INK_WHITE,
        atmos: [rgb8(0xe6, 0xe4, 0xee), rgb8(0xf6, 0xf5, 0xfa), rgb8(0xd8, 0xd0, 0xf4), rgb8(0xec, 0xea, 0xf2)],
        text: [
            rgb8(0x14, 0x12, 0x1e),
            rgb8(0x1f, 0x1c, 0x2b),
            rgb8(0x2a, 0x27, 0x38),
            rgb8(0x3a, 0x36, 0x48),
            rgb8(0x55, 0x51, 0x6a),
            rgb8(0x75, 0x71, 0x8a),
        ],
    }),
];

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn palette_named(name: &str) -> Palette {
    let mut i = 0;
    while i < PALETTES.len() {
        if str_eq(PALETTES[i].0, name) {
            return PALETTES[i].1;
        }
        i += 1;
    }
    panic!("STREMIO_THEME names no entry of gfx::tokens::PALETTES")
}

/// The theme this binary was built with: `STREMIO_THEME=<name>` at build time, else `stremio`.
// ponytail: a BUILD-time choice, so themes can be compared in the simulator without touching a
// call site; a user-facing setting needs these stops read at run time instead of as `const`s.
pub const THEME: &str = match option_env!("STREMIO_THEME") {
    Some(t) => t,
    None => "stremio",
};
pub const PALETTE: Palette = palette_named(THEME);
/// The scrim ink, near-black `#050508` (`theme::scrim`'s stop). It is NOT merged into `NEUTRAL_950`
/// (`#08080a`, two perceptual units lighter): the hero-text legibility floor
/// (`widgets::hero_scrim_tests::the_hero_text_reads_over_bright_artwork`) has 0.02:1 of headroom and the
/// lighter ink measured 2.48:1 against its 2.5:1.
pub const NEUTRAL_1000: [f32; 4] = rgb8(0x05, 0x05, 0x08);

/// GL clear color — 3-float (`frame_clear` takes r,g,b, no alpha). The app's DEFAULT base, and
/// `theme::SURFACE_APP` itself rather than a second copy of its code (both are [`NEUTRAL_500`]):
/// browsing screens clear to the app ground, which is what makes a route change read as a
/// seamless dip. Home overdraws it with `SURFACE_APP` (the identical colour). Two punches through to
/// the hardware video plane use transparent black instead: the player route, and the detail page
/// while a trailer preview has presented a frame (`gfx::frame_clear_through`).
pub const CLEAR_RGB: (f32, f32, f32) = (NEUTRAL_500[0], NEUTRAL_500[1], NEUTRAL_500[2]);

/// Hero/scroll scrim ink; use via `theme::scrim`.
pub const SCRIM_INK: [f32; 3] = [NEUTRAL_1000[0], NEUTRAL_1000[1], NEUTRAL_1000[2]];

/// Splat a token's rgb with an overridden alpha (e.g. the `env.sp`-baked hub title). Also how a role
/// spells a stop on the white/black **alpha ramps**: `with_a(WHITE, 0.20)`.
pub const fn with_a(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

// ── THE FOCUSED TILE'S LIT-GLASS EDGE (ArtTile component, Claude Design) ────────────────────────
// A FOCUSED art tile only — fades in with the focus pop `f` (0 at rest, 1 fully focused), folded
// into the same `fs_img.frag` pass as the resting sheen above rather than a second draw. Three
// layers, white light only, never a coloured ring or an outset border. The shader carries the
// pixel geometry as literals (it cannot read a Rust `const`); the numbers below are that shader's
// documentation copy, and `gfx.rs`'s `image_focus_geometry_matches_the_shader_literals` test pins
// the two together so one cannot drift from the other.
/// RIM inner glow (`--card-glass-rim-focus`), base layer: `inset 0 0 12px 0 white/.08`, carried
/// inward from the whole perimeter over this many px.
pub const CARD_GLOW_A: f32 = 0.08;
pub const CARD_GLOW_BAND_PX: f32 = 12.0;
/// RIM inner glow, TOP layer: `inset 0 4px 8px -4px white/.12` — a brighter, tighter band hugging
/// the top edge only (one light, from above — the same direction every card shadow falls in).
pub const CARD_GLOW_TOP_A: f32 = 0.12;
pub const CARD_GLOW_TOP_PX: f32 = 6.0;
/// RIM inner glow, BOTTOM layer: `inset 0 -4px 8px -5px white/.06` — fainter, tighter, on the
/// bottom edge (the light's own falloff reaching the far side of the tile).
pub const CARD_GLOW_BOT_A: f32 = 0.06;
pub const CARD_GLOW_BOT_PX: f32 = 4.0;
/// GLARE (`--card-glass-glare-focus`): the top tab bar's own glass crown. The resting 1px
/// perimeter sheen (`theme::CARD_SHEEN`, .22) is lifted to [`CARD_GLARE_A`] for the top `CARD_GLARE_PX`
/// of the tile height, easing linearly back to the plain sheen by `CARD_GLARE_EASE` of the height —
/// `linear-gradient(180deg, card_glare_a 0, card_glare_a 12px, transparent 16%)` masked to the 1px
/// ring. `CARD_GLARE_EASE` keeps the glare on the crown only, rather than running down the sides.
pub const CARD_GLARE_PX: f32 = 12.0;
pub const CARD_GLARE_EASE: f32 = 0.16;
/// The crown's own target alpha — brighter than `theme::GLASS_RIM_LIGHT` (.28) because a 1px hairline
/// over bright artwork needs more contrast than the same hairline over the dark glass track that
/// `theme::GLASS_RIM_LIGHT` was tuned for.
pub const CARD_GLARE_A: f32 = 0.45;
/// GLOSS (`--card-glass-gloss-focus`): `linear-gradient(160deg, white/.14 0%, transparent 34%)`
/// over the artwork — a soft top-left sheen on the face, composited under the RIM/GLARE above it.
pub const CARD_GLOSS_A: f32 = 0.14;
pub const CARD_GLOSS_FADE: f32 = 0.34;
/// The 160deg CSS gradient direction as a unit vector in card-local (x-right, y-down) space:
/// `(sin 160°, -cos 160°)` — mostly down, slightly left-to-right. `sin160 = sin20`, `-cos160 = cos20`.
pub const CARD_GLOSS_DIR: [f32; 2] = [0.342_020_14, 0.939_692_6];

// ── Type scale ───────────────────────────────────────────────────────────────
/// The one legibility-tuned ladder of text sizes for the whole UI — the *size* axis of the design
/// system (colours above, focus geometry below). Authored for a 1920×1080 panel viewed from a
/// couch, so [`size::CAPTION`] (24) is a **hard floor**: nothing in the product chrome renders
/// smaller, because sub-24 text is unreadable at that distance (the old raw 17/18/19/20/21 sizes
/// were exactly what read badly). Pass these to `Painter::text` / `Label` / `TextView` /
/// `text::elide` in place of a raw integer — a size is a *role*, not a magic number (mirrors the
/// "never a raw colour literal" rule). Rungs step ~1.15–1.25×; a role picks the nearest rung.
///
/// Two deliberate carve-outs sit outside the ladder (documented at their call site, not raw
/// literals): the player-HUD now-playing **display title** (larger than [`size::TITLE`]) and the
/// client-rendered **subtitle** caption — both media chrome with their own legibility contract, and
/// both already well above the floor. The boot splash (`anim.rs`) is likewise its own one-off.
///
/// **Rasterization contract:** the render path keeps every rung's stroke weights design-true
/// (light hinting in `text.rs::font_at`, pixel-snapped 1:1 quads via `gfx::snap`), so rung values
/// are chosen for hierarchy and legibility ONLY — no rung needs to dodge px sizes that hint badly.
/// After swapping fonts or touching hinting, re-verify with `tools/font-hint-audit.py`.
///
/// **A size cannot be animated — CROSSFADE two rungs instead.** Glyphs are cached per (size, bold)
/// as rasterized textures, so tweening a point size would rasterize a fresh run every frame and
/// churn that cache. A title that has to change size does it as two `Label` draws whose alphas run
/// opposite on one 0..1 progress: `detail.rs`'s hero → compact title (HERO → TITLE, on the scroll)
/// is how that is spelled. **`person.rs`'s band condense was the second worked example and is
/// gone** — that band stopped condensing (its module doc says why), so its name is one run at
/// `size::DISPLAY` now and a reader sent there for the pattern would conclude the rule is
/// unimplemented.
pub mod size {
    use std::os::raw::c_int;
    /// Full-bleed hero title — the home + detail hero headline.
    pub const HERO: c_int = 72;
    /// Full-card display title — the post-play (Up Next) card's episode name
    /// (`Plex Pass Awareness.dc.html` deliverable D): a one-line headline for a card that owns
    /// the FRAME but not the page, one ~1.2× ladder step past [`TITLE`] without [`HERO`]'s
    /// billboard weight (a 72px line over live credits would read as a new screen, not a prompt).
    pub const DISPLAY: c_int = 48;
    /// Screen / panel title — the in-player Info card title, the scrolled compact title, About column heads.
    pub const TITLE: c_int = 40;
    /// Section headers ("Related", "Cast & Crew") + list-row titles.
    pub const HEADLINE: c_int = 32;
    /// Default reading text (synopsis, hero meta, About paragraphs, empty states) + control labels (Play / action buttons, tabs, pills — one rung under a header).
    pub const BODY: c_int = 28;
    /// Secondary labels — card / episode / chapter titles, cast names, list detail, meta chips.
    pub const LABEL: c_int = 26;
    /// Couch legibility **FLOOR** for ordinary product text — kickers, timecodes, cast roles,
    /// badges and field labels. The deliberately opened diagnostics instrument is the sole
    /// exception; it has its own dense [`DIAGNOSTIC`] rung rather than weakening this product rule.
    pub const CAPTION: c_int = 24;
    /// Fine print — deliberately below the couch floor (explicit design direction, 2026-07-12,
    /// re-tuned on-device 16 → 20 → 22: "bigger, but smaller than the meta line"). De-emphasis is
    /// the point — atmosphere copy beside an outsized title, not content someone must read; labels
    /// the eye needs to catch (the SxEy kicker, meta lines) stay on the regular rungs.
    ///
    /// **NEITHER HERO'S SYNOPSIS IS ON THIS RUNG ANY MORE**, and this doc went on saying "the small
    /// info/synopsis text under the home/detail hero's title block" for both of the releases in
    /// which that was false of one of them and then of neither. Detail's blurb moved to `LABEL`/36
    /// in `aa598bf2`, home's followed when the two were made one block
    /// (`ui::hero_synopsis`, which holds the argument): a hero blurb is the longest run of
    /// prose on its page, so it is READING copy and the recorded "smaller than the meta line"
    /// directive is satisfied at `LABEL` 26 under a `BODY` 28 meta line. What is left here is the
    /// episode row's air date, the rating-row provider captions, the Library's note line, the
    /// player HUD's key capsule and the Source chip's handle — one-line labels, every one of them.
    pub const MICRO: c_int = 22;
    /// Dense engineering read-outs whose primary job is comparing many bounded numeric fields in
    /// one photograph.  This is deliberately below [`MICRO`]: diagnostics are opened on purpose
    /// and read as an instrument, not as ordinary couch copy.  It must never migrate into product
    /// chrome or prose; `app::diagnostics` is its sole owner.
    pub const DIAGNOSTIC: c_int = 20;
}
