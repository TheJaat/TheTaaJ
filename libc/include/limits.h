/* limits.h - how big the integer types are, and other limits.
 *
 * The C library's own, as stdint.h is: TCC, compiling on TheTaaJ, has
 * none, and both compilers should see the same numbers. LP64: int 32
 * bits, long 64. Whether char is signed differs between the machines -
 * signed on x86_64, unsigned on AArch64 - and the compilers say which
 * (__CHAR_UNSIGNED__). */
#ifndef _LIMITS_H
#define _LIMITS_H

#ifdef __cplusplus
extern "C" {
#endif

#define CHAR_BIT 8
#define SCHAR_MIN (-128)
#define SCHAR_MAX 127
#define UCHAR_MAX 255
#ifdef __CHAR_UNSIGNED__
#define CHAR_MIN 0
#define CHAR_MAX UCHAR_MAX
#else
#define CHAR_MIN SCHAR_MIN
#define CHAR_MAX SCHAR_MAX
#endif
#define MB_LEN_MAX 4

#define SHRT_MIN (-32767 - 1)
#define SHRT_MAX 32767
#define USHRT_MAX 65535
#define INT_MIN (-2147483647 - 1)
#define INT_MAX 2147483647
#define UINT_MAX 4294967295U
#define LONG_MIN (-9223372036854775807L - 1)
#define LONG_MAX 9223372036854775807L
#define ULONG_MAX 18446744073709551615UL
#define LLONG_MIN (-9223372036854775807LL - 1)
#define LLONG_MAX 9223372036854775807LL
#define ULLONG_MAX 18446744073709551615ULL

/* The biggest count read or write can return. */
#define SSIZE_MAX LONG_MAX

/* The room a path needs, its zero byte included. */
#define PATH_MAX 4096

#ifdef __cplusplus
}
#endif

#endif
