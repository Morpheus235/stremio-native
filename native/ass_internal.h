#ifndef SN_ASS_INTERNAL_H
#define SN_ASS_INTERNAL_H
/* Facade-internal helpers that need libass's private headers (src/ass_fonts.c). Not exported:
 * the library's version script only lets sn_ass_* leave. */
#include <ass/ass.h>
#include <stddef.h>

/* Fonts the library holds (added through ass_add_font, by the facade or by [Fonts] extraction). */
size_t sni_library_fonts(ASS_Library *library);
/* Add a font WITHOUT ass_add_font's copy: libass reads the caller's bytes, which must outlive the
 * library. 0, or -1 when libass could not add it. */
int sni_lend_font(ASS_Library *library, const char *name, const char *data, size_t len);
/* Before ass_library_done: forget the first `count` fonts' bytes (all lent), so libass frees
 * only what it owns. */
void sni_return_fonts(ASS_Library *library, size_t count);

#endif
