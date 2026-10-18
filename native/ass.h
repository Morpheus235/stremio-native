#ifndef SN_ASS_H
#define SN_ASS_H

/* Private, versioned facade over the pinned bundled libass. No upstream struct
 * crosses this ABI; every handle and returned pixel buffer belongs to one worker. */
#include <stddef.h>
#include <stdint.h>

typedef struct SnAss SnAss;
typedef struct {
    int x, y, width, height;
    size_t bytes;
    const uint8_t *rgba; /* straight RGBA; valid until the next render or destroy */
} SnAssBitmap;

#define SN_ASS_MAX_REGIONS 64
typedef struct {
    size_t count;
    const SnAssBitmap *regions; /* disjoint bounds; same lifetime as their pixels */
} SnAssFrame;

/* ABI 3: sn_ass_set_features, sn_ass_set_cache_limits, sn_ass_set_line_spacing,
 * sn_ass_replace_track and sn_ass_stats join the 2 surface. sn_ass_load now scans the fonts
 * (ass_set_fonts) only on a context's first load. */
/* ABI 4: sn_ass_add_font LENDS the bytes instead of copying them: they must stay valid until
 * sn_ass_destroy. */
#define SN_ASS_ABI_VERSION 4

typedef struct {
    uint32_t library_fonts; /* fonts the library holds: added through sn_ass_add_font or extracted */
    uint32_t font_scans;    /* times the renderer opened every font (ass_set_fonts) */
    uint32_t tracks;        /* tracks loaded or replaced */
} SnAssStats;

unsigned sn_ass_abi_version(void);
SnAss *sn_ass_create(const char *default_font);
void sn_ass_destroy(SnAss *ctx);
int sn_ass_add_font(SnAss *ctx, const char *name, const uint8_t *data, size_t len);
int sn_ass_load(SnAss *ctx, const uint8_t *data, size_t len, int script);
/* Replace the loaded track on the same library and renderer, keeping every font face the
 * renderer has opened. Fonts cannot change here: the new track's own [Fonts] section is NOT
 * extracted, so it must repeat the first track's (a synthesized header has none; a window of the
 * same script repeats the same section). Needs a first sn_ass_load. The next render returns a
 * full frame, never 0 "unchanged". -1 leaves the context without a track; such a context takes only
 * another sn_ass_replace_track (sn_ass_load refuses once the fonts are scanned). */
int sn_ass_replace_track(SnAss *ctx, const uint8_t *data, size_t len, int script);
/* Track features the facade re-applies on every load and replace. wrap_unicode: break lines by
 * the Unicode line-breaking rules (ASS_FEATURE_WRAP_UNICODE) rather than at spaces only.
 * -1 if the library cannot (built without libunibreak) or the context is unusable. */
int sn_ass_set_features(SnAss *ctx, int wrap_unicode);
/* libass glyph (count) and bitmap (MiB) cache bounds. -1 for a value below 1. */
int sn_ass_set_cache_limits(SnAss *ctx, int glyph_max, int bitmap_mib);
/* Extra pixels between lines at the render size; negative tightens. -1 if not finite or beyond
 * +-1000. The next render redraws in full. */
int sn_ass_set_line_spacing(SnAss *ctx, double spacing);
int sn_ass_stats(const SnAss *ctx, SnAssStats *out);
int sn_ass_chunk(SnAss *ctx, const uint8_t *data, size_t len,
                  int64_t start_ms, int64_t duration_ms);
void sn_ass_prune_before(SnAss *ctx, int64_t deadline_ms);
void sn_ass_flush_events(SnAss *ctx);
/* -1: refused/failed, 0: identical output (frame unchanged), 1: changed output.
 * A changed frame with count=0 clears the old subtitle. */
int sn_ass_render(SnAss *ctx, int64_t now_ms, int width, int height,
                   int storage_width, int storage_height,
                   SnAssFrame *frame);

#endif
