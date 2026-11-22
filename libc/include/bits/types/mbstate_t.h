/* bits/types/mbstate_t.h - mbstate_t on its own, for the headers that
 * need it: wchar.h and uchar.h, and C++'s library, which looks for it
 * here first (as on Linux) when it has no wide characters to include
 * wchar.h for. */
#ifndef _BITS_TYPES_MBSTATE_T_H
#define _BITS_TYPES_MBSTATE_T_H

/* The state of a conversion between multibyte characters (UTF-8) and
 * wide ones, between one call and the next: the bytes of a character
 * seen so far. All zero is the start. */
typedef struct {
    unsigned int count;   /* bytes seen of the character in progress */
    unsigned int partial; /* the bits they gave */
} mbstate_t;

#endif
