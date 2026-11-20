/* format.h - between the two halves of printf. Not for programs. */
#ifndef _TAAJ_FORMAT_H
#define _TAAJ_FORMAT_H

#include <stdarg.h>
#include <stddef.h>

/* Where formatted text goes: called with each piece, in order. */
typedef void (*emit_fn)(void *context, const char *bytes, size_t count);

/* Works through a printf format, handing the text to emit piece by
 * piece. Returns how many bytes that was. */
int __taaj_format(emit_fn emit, void *context, const char *format, va_list arguments);

#endif
