/* uchar.h - UTF-16 and UTF-32 characters (C11). Only the types so far:
 * mbstate_t, which C++'s library finds here when it is built without
 * wide characters, and char16_t and char32_t, which in C++ are words
 * of the language and need no typedef. */
#ifndef _UCHAR_H
#define _UCHAR_H

#include <bits/types/mbstate_t.h>

#ifndef __cplusplus
typedef unsigned short char16_t;
typedef unsigned int char32_t;
#endif

#endif
