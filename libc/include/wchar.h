/* wchar.h - wide characters. TheTaaJ's text is UTF-8 in plain chars,
 * and the C library has none of the wchar_t functions yet; this is
 * here for the types, which other headers use: C++'s strings keep an
 * mbstate_t ("where a multibyte conversion has got to") in every
 * stream position, wide characters or not. */
#ifndef _WCHAR_H
#define _WCHAR_H

#include <stddef.h>

#include <bits/types/mbstate_t.h>

/* A wide character, or WEOF: one more value than wchar_t holds. */
typedef unsigned int wint_t;
#define WEOF ((wint_t)-1)

/* wchar_t is a signed int on the PC and an unsigned one on AArch64. */
#ifdef __WCHAR_MAX__
#define WCHAR_MAX __WCHAR_MAX__
#if __WCHAR_MAX__ > 0x7fffffff
#define WCHAR_MIN 0U
#else
#define WCHAR_MIN (-__WCHAR_MAX__ - 1)
#endif
#endif

#endif
