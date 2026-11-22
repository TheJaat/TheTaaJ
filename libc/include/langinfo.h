/* langinfo.h - single facts about the locale, by number: nl_langinfo
 * (CODESET) is the name of its character set. Of the one locale, "C".
 * (libc/c/locale.c) */
#ifndef _LANGINFO_H
#define _LANGINFO_H

#ifdef __cplusplus
extern "C" {
#endif

typedef int nl_item;

#define CODESET   0 /* "ASCII": "C" has a byte per character */
#define RADIXCHAR 1 /* "." */
#define THOUSEP   2 /* "" */
#define D_T_FMT   3 /* strftime's format for %c */
#define D_FMT     4 /* for %x */
#define T_FMT     5 /* for %X */

/* The fact asked for, as a string that belongs to the library; "" for
 * an item it does not know. */
char *nl_langinfo(nl_item item);

#ifdef __cplusplus
}
#endif

#endif
