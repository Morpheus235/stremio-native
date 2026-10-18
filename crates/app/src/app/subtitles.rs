//! **The subtitles drawn over the video plane** — the player page's subtitle layer
//! (`sn_screens::player::install_subtitle_painter`), which the screens cannot draw themselves
//! because it reads the media layer. Ported from PlxNative (`screens::player::ass_subtitles` and
//! `appkit::player_hud`'s image-subtitle and message draws):
//!
//! - **text and styled subtitles** go through libass (`sn_media::player::ass`): the frame thread
//!   submits its clock and uploads only a changed completed image; a text subtitle's bottom cues
//!   rise clear of the transport while it is up (plan `libass-text-subs` §2.4), an authored
//!   script keeps its own placement;
//! - **image subtitles** (PGS, VobSub) are the decoded display set, scaled from the stream's
//!   canvas onto the screen and lifted clear of the transport the same way;
//! - a subtitle that cannot be shown (libass missing or failing, a sidecar that produced
//!   nothing) says so in the caption's place.
//!
//! [`update`] runs once a loop iteration (`Playback::tick`) and answers whether what is drawn
//! changed — the present gate's reason to draw; [`paint`] runs inside the player page's draw.
//! Both are main-thread: the state is a thread-local and holds GL names.

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::{c_int, c_uint};
use std::sync::Arc;

use sn_machine::machine::Measure;
use sn_media::player::text_style::{
    position_lift, size_scale, wrap_width, CAPTION_ANCHOR_LINES, CAPTION_FACE, CAPTION_FLOOR_Y, CAPTION_LINE_PITCH,
    CAPTION_TEXT_INSET, SUB_BASE_Y, SUB_CEIL_Y,
};
use sn_media::player::{ass, ass_source, sidecar, text_style, video_geometry::Viewport};
use sn_ui::consts::{SCR_H, SCR_W};
use sn_ui::{theme, Painter, Rect};

/// Everything the layer holds: the libass frame and its textures, the image set.
#[derive(Default)]
struct Layer {
    ass: AssSubtitles,
    bitmaps: SubtitleBitmaps,
    /// What [`update`] last saw, so a change is a frame.
    shown: (u64, i64, bool),
}

thread_local! {
    static LAYER: RefCell<Layer> = RefCell::new(Layer::default());
}

/// Once a loop iteration, playing or not: ask libass for the frame at the playhead. `true` when
/// what the layer draws changed (a cue appeared, cleared, a notice came or went).
pub fn update(ps: &sn_media::session::PlaybackSession, now: u32) -> bool {
    LAYER.with(|l| {
        let mut l = l.borrow_mut();
        l.ass.update(ps, now);
        let key = sn_media::player::active_bitmap_key(sn_media::player::playpos_ns()).unwrap_or(i64::MIN);
        let shown = (l.ass.fingerprint(), key ^ i64::from(sn_media::player::desired_sub_idx()), sidecar::notice_shown());
        std::mem::replace(&mut l.shown, shown) != shown
    })
}

/// The player page's subtitle layer (installed as its `SubtitlePainter`).
pub fn paint(hud_up: bool, measure: &dyn Measure) {
    LAYER.with(|l| {
        let mut l = l.borrow_mut();
        l.ass.draw(hud_up);
        if let Some(message) = l.ass.error() {
            draw_subtitle_message(message, hud_up, measure);
        }
        // a sidecar that produced nothing says so for a few seconds
        if let Some(message) = sidecar::notice() {
            draw_subtitle_message(message, hud_up, measure);
        }
        draw_subtitle_bitmap(&mut l.bitmaps, hud_up);
    });
}

/// The playback ended: give the GL names back.
pub fn release() {
    LAYER.with(|l| {
        let mut l = l.borrow_mut();
        l.ass.release();
        l.bitmaps.release();
        l.shown = (0, i64::MIN, false);
    });
}

/// The ink both subtitle draws use: the viewer's tone on [`theme::SUBTITLE_INKS`] (a rung the
/// table does not have is white, never a neighbour).
fn subtitle_ink() -> [f32; 4] {
    theme::SUBTITLE_INKS
        .get(usize::from(sn_media::player::subtitle_tone().index()))
        .copied()
        .unwrap_or(theme::SUBTITLE_INKS[0])
}

// ---- the caption-place message --------------------------------------------------------------

/// A short fixed message in the caption's place, at the viewer's size and position.
fn draw_subtitle_message(text: &str, hud_up: bool, measure: &dyn Measure) {
    let (scale, baseline, floor) = caption_geometry(hud_up);
    let lines = caption_wrap(text, scale, measure);
    if lines.is_empty() {
        return;
    }
    let lay = caption_layout(lines.len(), scale, baseline, floor);
    let cx = SCR_W * 0.5;
    let ink = subtitle_ink();
    let outline = theme::scrim_black(0.85);
    let p = Painter::root();
    for (i, ln) in lines.iter().enumerate() {
        let top = lay.top + i as f32 * lay.pitch;
        if let Ok(cs) = CString::new(ln.as_str()) {
            // dark outline (4 offsets) then the bold caption in its tone — legible over any scene
            for (dx, dy) in [(-2.0f32, 0.0f32), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
                p.text(cs.as_ptr(), cx + dx, top + CAPTION_TEXT_INSET + dy, lay.face, outline, 1, 1);
            }
            p.text(cs.as_ptr(), cx, top + CAPTION_TEXT_INSET, lay.face, ink, 1, 1);
        }
    }
}

/// The viewer's size, the y a caption block is placed from (lifted above the transport while it
/// is up, and by the viewer's Position pick), and the lowest y the block may reach.
fn caption_geometry(hud_up: bool) -> (f32, f32, f32) {
    let scale = size_scale(sn_media::prefs::subtitle_size());
    let baseline = (if hud_up { SUB_CEIL_Y } else { SUB_BASE_Y }) - position_lift(sn_media::prefs::subtitle_position());
    let floor = if hud_up { SUB_CEIL_Y } else { CAPTION_FLOOR_Y };
    (scale, baseline, floor)
}

fn caption_wrap(text: &str, scale: f32, measure: &dyn Measure) -> Vec<String> {
    let face = (CAPTION_FACE * scale).round() as c_int;
    let width = wrap_width(scale);
    let mut lines = Vec::new();
    for seg in text.split('\n').map(str::trim).filter(|s| !s.is_empty()) {
        let mut cur = String::new();
        for word in seg.split_whitespace() {
            if !cur.is_empty() && measure.width_str(&format!("{cur} {word}"), face, true) > width {
                lines.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

struct CaptionLayout {
    face: i32,
    pitch: f32,
    top: f32,
}

/// One block of `lines` lines, its bottom on `baseline` for up to [`CAPTION_ANCHOR_LINES`]; a
/// block that would pass `floor` steps its size down until it fits.
fn caption_layout(lines: usize, scale: f32, baseline: f32, floor: f32) -> CaptionLayout {
    let anchored = lines.min(CAPTION_ANCHOR_LINES) as f32;
    let top = baseline - anchored * CAPTION_LINE_PITCH * scale;
    let mut lay = CaptionLayout { face: 0, pitch: 0.0, top };
    for step in [1.0, 0.875, 0.75, 0.625] {
        let s = scale * step;
        lay = CaptionLayout { face: (CAPTION_FACE * s).round() as i32, pitch: CAPTION_LINE_PITCH * s, top };
        if top + lines as f32 * lay.pitch <= floor {
            break;
        }
    }
    lay
}

// ---- image subtitles --------------------------------------------------------------------------

/// The decoded image-subtitle display set on screen and its cache key `(track, start)`.
struct SubtitleBitmaps {
    set: Vec<(c_uint, Rect)>,
    key: i64,
    sel: i32,
}

impl Default for SubtitleBitmaps {
    fn default() -> Self {
        SubtitleBitmaps { set: Vec::new(), key: i64::MIN, sel: i32::MIN }
    }
}

impl SubtitleBitmaps {
    fn release(&mut self) {
        for (t, _) in self.set.drain(..) {
            sn_gfx::gfx::delete_tex(t);
        }
        self.key = i64::MIN;
        self.sel = i32::MIN;
    }
}

/// A decoded rect from the stream's `cw`×`ch` canvas onto the screen (1:1 when no canvas).
fn sub_screen_rect(r: (i32, i32, i32, i32), cw: i32, ch: i32) -> Rect {
    let sx = if cw > 0 { SCR_W / cw as f32 } else { 1.0 };
    let sy = if ch > 0 { SCR_H / ch as f32 } else { 1.0 };
    Rect::new(r.0 as f32 * sx, r.1 as f32 * sy, r.2 as f32 * sx, r.3 as f32 * sy)
}

/// Does this rect reach below the transport ceiling? Only such rects take the lift.
fn overhangs(r: Rect) -> bool {
    r.y + r.h > SUB_CEIL_Y
}

/// How far up the overhanging rects move, as one group, while the HUD is up — never past the
/// group's own top.
fn hud_lift(rects: impl Iterator<Item = Rect>, hud_up: bool) -> f32 {
    if !hud_up {
        return 0.0;
    }
    let (mut top, mut bottom) = (f32::MAX, f32::MIN);
    for r in rects.filter(|r| overhangs(*r)) {
        top = top.min(r.y);
        bottom = bottom.max(r.y + r.h);
    }
    if top > bottom {
        return 0.0;
    }
    (bottom - SUB_CEIL_Y).max(0.0).min(top.max(0.0))
}

/// Composite the active image display set; textures are re-uploaded only when the cue changes.
fn draw_subtitle_bitmap(cache: &mut SubtitleBitmaps, hud_up: bool) {
    let sel = sn_media::player::desired_sub_idx();
    if sel < 0 {
        cache.release();
        return;
    }
    let Some(k) = sn_media::player::active_bitmap_key(sn_media::player::playpos_ns()) else {
        cache.key = i64::MIN; // between cues
        return;
    };
    if k != cache.key || sel != cache.sel {
        let Some((cw, ch, rects)) = sn_media::player::bitmap_by_key(k) else { return };
        for (t, _) in cache.set.drain(rects.len().min(cache.set.len())..) {
            sn_gfx::gfx::delete_tex(t);
        }
        for (i, r) in rects.iter().enumerate() {
            let dst = sub_screen_rect((r.x, r.y, r.w, r.h), cw, ch);
            let prev = cache.set.get(i).map_or(0, |(t, _)| *t);
            let rgba = r.to_rgba();
            let tex = sn_gfx::gfx::upload_rgba(prev, r.w, r.h, rgba.as_ptr());
            match cache.set.get_mut(i) {
                Some(slot) => *slot = (tex, dst),
                None => cache.set.push((tex, dst)),
            }
        }
        cache.key = k;
        cache.sel = sel;
    }
    let lift = hud_lift(cache.set.iter().map(|(_, r)| *r), hud_up);
    // the tone TINTS a bitmap: it keeps its authored colours and only gives up light
    let tint = subtitle_ink();
    let p = Painter::root();
    for (tex, r) in &cache.set {
        let dy = if overhangs(*r) { lift } else { 0.0 };
        p.tex(*tex, Rect::new(r.x, r.y - dy, r.w, r.h), 0.0, tint);
    }
}

// ---- libass ---------------------------------------------------------------------------------

#[derive(Default)]
pub struct AssSubtitles {
    frame: Option<Arc<ass::Frame>>,
    source: u64,
    clock: ass_source::Clock,
    textures: Vec<CachedTexture>,
    uploaded: u64,
    viewport: Viewport,
    /// The origin `frame` was requested under. The source's current origin can already differ
    /// (a restyle bumps the revision when its worker publishes; the libass worker's re-render
    /// follows), and the lift is
    /// decided by the frame on screen (plan `libass-text-subs` §2.4).
    frame_origin: ass::Origin,
    /// The alignment classes of the cues in one frame, by its serial (Text origin only).
    aligns: Option<(u64, ass::AlignSet)>,
    /// Reused each draw so the 60 fps path allocates nothing.
    spans: Vec<(i32, i32)>,
    dys: Vec<f32>,
}

/// Every class present: the rule for a frame whose own classes are not known. It never lifts a
/// top or middle cue and still lifts a normal bottom block by the full `HUD_LIFT`.
const UNKNOWN_ALIGNS: ass::AlignSet = ass::AlignSet { bottom: true, middle: true, top: true };

impl AssSubtitles {
    pub fn update(&mut self, ps: &sn_media::session::PlaybackSession, now: u32) {
        let source = if sn_media::player::loading(ps) {
            None
        } else {
            sidecar::ass_source().or_else(|| ass_source::selected(sn_media::player::desired_sub_idx()))
        };
        let Some(source) = source else {
            if self.source != 0 {
                ass::clear();
            }
            self.source = 0;
            self.frame = None;
            return;
        };
        if self.source != source.id {
            self.source = source.id;
            self.clock = ass_source::Clock::default();
            self.frame = None;
            self.aligns = None;
        }
        let clock = self.clock.sample(
            sn_media::player::playpos_ns(),
            now,
            sn_media::player::is_playing(ps),
        );
        let clock = clock.saturating_sub(sn_media::player::subtitle_offset_ms());
        let (viewport, (coded_w, coded_h)) = frame_geometry(
            source.origin,
            sn_media::player::video_viewport(SCR_W as i32, SCR_H as i32),
            sn_media::player::video_raster(),
        );
        if self.viewport != viewport {
            self.viewport = viewport;
            self.frame = None;
        }
        let origin = source.origin;
        if let Some(frame) =
            ass::request(source, clock, viewport.width, viewport.height, coded_w, coded_h)
        {
            self.frame = Some(frame);
            self.frame_origin = origin;
        }
    }

    pub fn fingerprint(&self) -> u64 {
        self.frame.as_ref().map_or(0, |f| f.serial)
    }

    /// Present a frame with `serial` and no regions, as the worker's answer to a cue change would:
    /// what the player's frame-fingerprint test moves.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn set_frame_serial_for_test(&mut self, serial: u64) {
        self.frame = Some(Arc::new(ass::Frame {
            source_id: self.source,
            serial,
            width: 0,
            height: 0,
            rects: Vec::new(),
            error: None,
        }));
    }

    /// `lift` is the transport HUD being up. It moves a text subtitle's bottom regions and nothing
    /// else: an authored script keeps its own placement.
    pub fn draw(&mut self, lift: bool) {
        let Some(frame) = self.frame.clone() else {
            if self.source == 0 {
                clear_textures(&mut self.textures);
                self.uploaded = 0;
            }
            return;
        };
        if self.uploaded != frame.serial {
            sn_base::diag::spans::span("assup", || {
                // Reserve all unchanged regions first. A changing region must not steal
                // a texture that a later static sign still needs in this same frame.
                let matches = retained_regions(&self.textures, &frame.rects);
                let mut old: Vec<_> = std::mem::take(&mut self.textures)
                    .into_iter()
                    .map(Some)
                    .collect();
                let mut next: Vec<_> = matches
                    .into_iter()
                    .map(|index| index.and_then(|i| old[i].take()))
                    .collect();
                for (slot, rect) in next.iter_mut().zip(&frame.rects) {
                    if slot.is_none() {
                        let previous = old.iter_mut().find_map(Option::take);
                        sn_media::player::ass_perf::note_upload(rect.rgba.len());
                        let id = sn_gfx::gfx::upload_rgba(
                            previous.as_ref().map_or(0, |t| t.id),
                            rect.width,
                            rect.height,
                            rect.rgba.as_ptr(),
                        );
                        *slot = Some(CachedTexture {
                            id,
                            width: rect.width,
                            height: rect.height,
                            pixels: rect.rgba.clone(),
                        });
                    }
                }
                for unused in old.into_iter().flatten() {
                    sn_gfx::gfx::delete_tex(unused.id);
                }
                self.textures = next.into_iter().flatten().collect();
                self.uploaded = frame.serial;
            });
        }
        let origin = self.frame_origin;
        let aligns = if lift && origin.is_text() {
            aligns_for(&mut self.aligns, frame.serial, || ass::published_aligns(frame.serial))
        } else {
            UNKNOWN_ALIGNS
        };
        self.spans.clear();
        self.spans.extend(frame.rects.iter().map(|r| (r.y, r.height)));
        lift_dys(origin, lift, &self.spans, aligns, &mut self.dys);
        let ink = subtitle_ink();
        for (i, (texture, rect)) in self.textures.iter().zip(&frame.rects).enumerate() {
            let dy = self.dys[i];
            Painter::root().tex(
                texture.id,
                Rect::new(
                    (self.viewport.x + rect.x) as f32,
                    (self.viewport.y + rect.y) as f32 + dy,
                    rect.width as f32,
                    rect.height as f32,
                ),
                0.0,
                ink,
            );
        }
    }

    pub fn error(&self) -> Option<&'static str> {
        self.frame.as_ref().and_then(|f| f.error).map(sn_media::player::ass::Fault::message)
    }

    pub fn release(&mut self) {
        clear_textures(&mut self.textures);
        self.uploaded = 0;
        self.frame = None;
        self.aligns = None;
        self.source = 0;
        ass::clear();
    }

}

/// The viewport a frame is rendered into and the storage raster it is sized against. An authored
/// script keeps the video's; a text subtitle is placed in screen space, so it gets the whole
/// logical screen whatever the video's aspect (plan §2.1).
fn frame_geometry(
    origin: ass::Origin,
    video: Viewport,
    coded: (i32, i32),
) -> (Viewport, (i32, i32)) {
    if origin.is_text() {
        let (w, h) = (text_style::SCR_W as i32, text_style::SCR_H as i32);
        (Viewport { x: 0, y: 0, width: w, height: h }, (w, h))
    } else {
        (video, coded)
    }
}

/// How far (vertical px, negative is up) each region of a Text-origin frame moves while the HUD is
/// up (`lift`); an authored frame never moves. `regions` are `(y, height)` in frame pixels. Plan §2.4:
/// - every active cue bottom-aligned: all regions move by the fixed `HUD_LIFT`;
/// - no bottom cue: nothing moves;
/// - mixed: a region moves when its bottom edge is at or below `anchor - pitch / 2`, unless such a
///   region also starts above the frame's midline (a merged top+bottom region), in which case
///   nothing moves, erring towards not lifting a top cue. A region right above the moving group
///   (a block whose lines are separate regions) joins it.
///
/// The moved group is clamped once so its top stays on the screen.
fn lift_dys(
    origin: ass::Origin,
    lift: bool,
    regions: &[(i32, i32)],
    aligns: ass::AlignSet,
    dys: &mut Vec<f32>,
) {
    dys.clear();
    dys.resize(regions.len(), 0.0);
    let ass::Origin::Text(style) = origin else { return };
    if !lift || !aligns.bottom {
        return;
    }
    let pitch = text_style::CAPTION_LINE_PITCH * style.scale();
    // `dys` doubles as the "moves" mark (-1) until the lift is known.
    if aligns.middle || aligns.top {
        let band = text_style::bottom_anchor_y(style.position) - pitch / 2.0;
        let mut merged = false;
        for (dy, &(y, h)) in dys.iter_mut().zip(regions) {
            if (y + h) as f32 >= band {
                *dy = -1.0;
                merged |= (y as f32) < text_style::SCR_H / 2.0;
            }
        }
        if merged {
            dys.fill(0.0);
            return;
        }
        // A block whose lines are separate regions rises as one: a region sitting right above
        // the moving group (a gap under half a pitch) joins it, up to a fixed point.
        while let Some(top) = moving_top(regions, dys) {
            let mut grew = false;
            for (dy, &(y, h)) in dys.iter_mut().zip(regions) {
                if *dy == 0.0 && y < top && ((y + h) as f32) >= top as f32 - pitch / 2.0 {
                    *dy = -1.0;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
    } else {
        dys.fill(-1.0);
    }
    let Some(top) = moving_top(regions, dys) else { return };
    let lift = text_style::HUD_LIFT.min(top.max(0) as f32);
    for dy in dys.iter_mut().filter(|d| **d != 0.0) {
        *dy = -lift;
    }
}

/// The top edge of the regions marked as moving (`dys` entry non-zero).
fn moving_top(regions: &[(i32, i32)], dys: &[f32]) -> Option<i32> {
    regions.iter().zip(dys).filter(|(_, &d)| d != 0.0).map(|(r, _)| r.0).min()
}

/// The classes of the frame with `serial`: the cached answer if it is for this frame, else ask
/// (`ask` may find the mailbox busy, or the frame no longer published) and cache a hit. With no
/// answer for THIS frame the classes are unknown, never another frame's (`UNKNOWN_ALIGNS`).
fn aligns_for(
    cache: &mut Option<(u64, ass::AlignSet)>,
    serial: u64,
    ask: impl FnOnce() -> Option<ass::AlignSet>,
) -> ass::AlignSet {
    if let Some((s, set)) = *cache {
        if s == serial {
            return set;
        }
    }
    match ask() {
        Some(set) => {
            *cache = Some((serial, set));
            set
        }
        None => UNKNOWN_ALIGNS,
    }
}

struct CachedTexture {
    id: u32,
    width: i32,
    height: i32,
    pixels: Arc<[u8]>,
}

fn clear_textures(textures: &mut Vec<CachedTexture>) {
    for texture in textures.drain(..) {
        sn_gfx::gfx::delete_tex(texture.id);
    }
}

/// The worker interns unchanged RGBA. Pixel identity can therefore retain an
/// upload even if a sign moved or libass reordered its disjoint output regions.
fn retained_regions(textures: &[CachedTexture], rects: &[ass::Rect]) -> Vec<Option<usize>> {
    let mut used = vec![false; textures.len()];
    rects
        .iter()
        .map(|rect| {
            let found = textures.iter().enumerate().position(|(i, texture)| {
                !used[i]
                    && texture.width == rect.width
                    && texture.height == rect.height
                    && Arc::ptr_eq(&texture.pixels, &rect.rgba)
            });
            if let Some(i) = found {
                used[i] = true;
            }
            found
        })
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;
    use sn_media::player::text_style::TextStyle;
    use sn_media::prefs::{SubtitlePosition, SubtitleSize};

    const BOTTOM: ass::AlignSet = ass::AlignSet { bottom: true, middle: false, top: false };
    const TOP_AND_BOTTOM: ass::AlignSet = ass::AlignSet { bottom: true, middle: false, top: true };

    fn dys_of(
        origin: ass::Origin,
        lift: bool,
        regions: &[(i32, i32)],
        aligns: ass::AlignSet,
    ) -> Vec<f32> {
        let mut dys = Vec::new();
        lift_dys(origin, lift, regions, aligns, &mut dys);
        dys
    }

    fn text(style: TextStyle) -> ass::Origin {
        ass::Origin::Text(style)
    }

    fn style(size: SubtitleSize, position: SubtitlePosition) -> TextStyle {
        TextStyle { size, position, ..Default::default() }
    }

    /// A one-line region whose bottom edge sits on the style's bottom anchor, `extra` px below
    /// the line (a descender).
    fn line_at(style: TextStyle, extra: i32) -> (i32, i32) {
        let pitch = (text_style::CAPTION_LINE_PITCH * style.scale()) as i32;
        let bottom = text_style::bottom_anchor_y(style.position) as i32 + extra;
        (bottom - pitch, pitch)
    }

    #[test]
    fn text_origin_renders_into_the_whole_screen() {
        let video = Viewport { x: 0, y: 140, width: 1920, height: 800 };
        let (viewport, storage) = frame_geometry(
            ass::Origin::Text(TextStyle::default()),
            video,
            (3840, 1600),
        );
        assert_eq!(viewport, Viewport { x: 0, y: 0, width: 1920, height: 1080 });
        assert_eq!(storage, (1920, 1080));
        let (viewport, storage) = frame_geometry(ass::Origin::Authored, video, (3840, 1600));
        assert_eq!((viewport, storage), (video, (3840, 1600)));
    }

    #[test]
    fn bottom_cues_rise_exactly_the_hud_lift_at_every_position_and_size() {
        for position in SubtitlePosition::LADDER {
            for size in SubtitleSize::LADDER {
                let style = style(size, position);
                // With and without descenders: the same motion, the descender does not jitter it.
                let plain = line_at(style, 0);
                let deep = line_at(style, 12);
                let dys = dys_of(ass::Origin::Text(style), true, &[plain, deep], BOTTOM);
                assert_eq!(dys, [-200.0, -200.0], "{position:?} {size:?}");
                assert_eq!(text_style::HUD_LIFT, 200.0);
            }
        }
    }

    #[test]
    fn an_authored_frame_is_unmoved_with_the_hud_up() {
        let style = TextStyle::default();
        let regions = [line_at(style, 0), (100, 48), line_at(style, 12)];
        for aligns in [BOTTOM, TOP_AND_BOTTOM, ass::AlignSet::default()] {
            assert_eq!(
                dys_of(ass::Origin::Authored, true, &regions, aligns),
                [0.0, 0.0, 0.0]
            );
        }
    }

    #[test]
    fn a_text_frame_is_unmoved_with_the_hud_down() {
        let style = TextStyle::default();
        assert_eq!(dys_of(text(style), false, &[line_at(style, 0)], BOTTOM), [0.0]);
    }

    #[test]
    fn a_four_line_block_moves_as_one() {
        let style = style(SubtitleSize::ExtraLarge, SubtitlePosition::High);
        let pitch = 72;
        let bottom = text_style::bottom_anchor_y(style.position) as i32;
        let region = (bottom - 4 * pitch, 4 * pitch);
        assert_eq!(dys_of(ass::Origin::Text(style), true, &[region], BOTTOM), [-200.0]);
    }

    #[test]
    fn nothing_moves_without_a_bottom_cue() {
        let style = TextStyle::default();
        let middle = ass::AlignSet { bottom: false, middle: true, top: false };
        let top = ass::AlignSet { bottom: false, middle: false, top: true };
        let r = [line_at(style, 0), (500, 48)];
        assert_eq!(dys_of(text(style), true, &r, middle), [0.0, 0.0]);
        assert_eq!(dys_of(text(style), true, &r, top), [0.0, 0.0]);
        assert_eq!(dys_of(text(style), true, &r, ass::AlignSet::default()), [0.0, 0.0]);
    }

    #[test]
    fn only_the_bottom_cue_moves_beside_a_top_one() {
        for position in SubtitlePosition::LADDER {
            for size in SubtitleSize::LADDER {
                let style = style(size, position);
                let top_cue = (100, 48);
                let bottom_cue = line_at(style, 12);
                assert_eq!(
                    dys_of(text(style), true, &[top_cue, bottom_cue], TOP_AND_BOTTOM),
                    [0.0, -200.0],
                    "{position:?} {size:?}"
                );
            }
        }
    }

    #[test]
    fn a_merged_top_and_bottom_region_is_not_lifted() {
        for position in SubtitlePosition::LADDER {
            for size in SubtitleSize::LADDER {
                let style = style(size, position);
                let bottom = text_style::bottom_anchor_y(style.position) as i32;
                let merged = (100, bottom - 100);
                assert_eq!(
                    dys_of(text(style), true, &[merged], TOP_AND_BOTTOM),
                    [0.0],
                    "{position:?} {size:?}"
                );
            }
        }
    }

    #[test]
    fn a_four_line_block_as_separate_regions_moves_together_beside_a_top_cue() {
        let style = style(SubtitleSize::ExtraLarge, SubtitlePosition::High);
        let pitch = (text_style::CAPTION_LINE_PITCH * style.scale()) as i32;
        let bottom = text_style::bottom_anchor_y(style.position) as i32;
        let mut regions = vec![(100, 48)];
        regions.extend((1..=4).rev().map(|n| (bottom - n * pitch, pitch)));
        assert_eq!(
            dys_of(text(style), true, &regions, TOP_AND_BOTTOM),
            [0.0, -200.0, -200.0, -200.0, -200.0]
        );
    }

    #[test]
    fn unknown_classes_never_lift_a_top_cue_but_lift_a_normal_bottom_block() {
        for position in SubtitlePosition::LADDER {
            for size in SubtitleSize::LADDER {
                let style = style(size, position);
                // The middle cue sits more than half a Huge pitch (43 px) above the High block's
                // line, so the lift cannot take it for one of that block's lines.
                let dys = dys_of(
                    text(style),
                    true,
                    &[(100, 48), (450, 48), line_at(style, 12)],
                    UNKNOWN_ALIGNS,
                );
                assert_eq!(dys, [0.0, 0.0, -200.0], "{position:?} {size:?}");
            }
        }
    }

    #[test]
    fn a_frame_without_an_answer_does_not_borrow_another_frames_classes() {
        let mut cache = None;
        assert_eq!(aligns_for(&mut cache, 7, || Some(BOTTOM)), BOTTOM);
        // Cached for this serial: no ask.
        assert_eq!(aligns_for(&mut cache, 7, || panic!("asked again")), BOTTOM);
        // Another frame, mailbox busy or frame gone: unknown, not the bottom-only set.
        assert_eq!(aligns_for(&mut cache, 8, || None), UNKNOWN_ALIGNS);
        assert_eq!(aligns_for(&mut cache, 8, || Some(TOP_AND_BOTTOM)), TOP_AND_BOTTOM);
        // The top region does not move under the unknown set.
        let style = TextStyle::default();
        let regions = [(100, 48), line_at(style, 0)];
        assert_eq!(dys_of(text(style), true, &regions, UNKNOWN_ALIGNS), [0.0, -200.0]);
    }

    #[test]
    fn the_lifted_group_stays_on_the_screen() {
        let style = TextStyle::default();
        // A tall block whose top is 60 px down moves only 60.
        assert_eq!(dys_of(text(style), true, &[(60, 900)], BOTTOM), [-60.0]);
        assert_eq!(dys_of(text(style), true, &[], BOTTOM), Vec::<f32>::new());
    }

    #[test]
    fn moving_and_reordered_regions_retain_uploads_before_recycling_changed_ones() {
        let a: Arc<[u8]> = Arc::from([255, 0, 0, 255]);
        let b: Arc<[u8]> = Arc::from([0, 0, 255, 255]);
        let old = vec![
            CachedTexture {
                id: 11,
                width: 1,
                height: 1,
                pixels: a.clone(),
            },
            CachedTexture {
                id: 12,
                width: 1,
                height: 1,
                pixels: b.clone(),
            },
        ];
        let rect = |x, rgba| ass::Rect {
            x,
            y: 0,
            width: 1,
            height: 1,
            rgba,
        };
        let changed: Arc<[u8]> = Arc::from([0, 255, 0, 255]);
        let next = vec![rect(40, b), rect(90, changed), rect(120, a.clone())];
        assert_eq!(retained_regions(&old, &next), [Some(1), None, Some(0)]);
        assert_eq!(
            retained_regions(&old, &[rect(0, a.clone()), rect(20, a)]),
            [Some(0), None]
        );
        assert!(retained_regions(&old, &[]).is_empty());
    }

    #[test]
    fn an_image_set_lifts_only_what_reaches_the_transport_and_as_one() {
        let sign = Rect::new(0.0, 100.0, 200.0, 50.0);
        let dialogue = Rect::new(0.0, SCR_H - 120.0, 800.0, 100.0);
        assert_eq!(hud_lift([sign, dialogue].into_iter(), false), 0.0, "nothing moves with the HUD down");
        let lift = hud_lift([sign, dialogue].into_iter(), true);
        assert_eq!(lift, dialogue.y + dialogue.h - SUB_CEIL_Y, "the dialogue clears the transport");
        assert!(!overhangs(sign), "the sign stays where it was authored");
        assert_eq!(sub_screen_rect((0, 0, 720, 480), 720, 480), Rect::new(0.0, 0.0, SCR_W, SCR_H), "a DVD canvas fills the screen");
    }

    #[test]
    fn a_message_steps_down_rather_than_passing_its_floor() {
        let lay = caption_layout(2, 1.0, SUB_BASE_Y, CAPTION_FLOOR_Y);
        assert_eq!(lay.face, CAPTION_FACE.round() as i32, "a short block keeps the viewer's size");
        let tall = caption_layout(8, 1.0, SUB_CEIL_Y, SUB_CEIL_Y);
        assert!(tall.face < CAPTION_FACE as i32, "a block too tall for its room steps down");
    }

    /// The screens' control row is the line captions rise clear of.
    #[test]
    fn the_hud_row_is_the_media_layers() {
        assert_eq!(sn_screens::player::CTRL_Y, text_style::HUD_CONTROLS_TOP_Y);
    }
}
