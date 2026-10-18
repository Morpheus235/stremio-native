/* Compiled INTO libass-sn. The one place the facade reads a private libass structure: the
 * pinned build installs ass_library.h next to the public headers (ci/build-libass.py) so a test
 * can prove that embedded fonts are not extracted again on every replaced track, and so a font
 * the app already holds is lent to libass instead of copied. */
#include "ass_internal.h"
#include "ass_library.h"

#include <stdlib.h>

size_t sni_library_fonts(ASS_Library *library)
{
    return library ? library->num_fontdata : 0;
}

int sni_lend_font(ASS_Library *library, const char *name, const char *data, size_t len)
{
    size_t idx = library->num_fontdata;
    /* ass_add_font grows the table and copies the name; one byte of data stands in for the copy
     * it would make of the font, and is swapped for the caller's bytes. */
    ass_add_font(library, name, data, 1);
    if (library->num_fontdata != idx + 1) return -1;
    free(library->fontdata[idx].data);
    library->fontdata[idx].data = (char *)data;
    library->fontdata[idx].size = (int)len;
    return 0;
}

void sni_return_fonts(ASS_Library *library, size_t count)
{
    for (size_t i = 0; i < count && i < library->num_fontdata; ++i)
        library->fontdata[i].data = NULL;
}
