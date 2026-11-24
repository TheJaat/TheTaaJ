/* stdlib.h - the general-purpose part of the C library. */
#ifndef _STDLIB_H
#define _STDLIB_H

#ifdef __cplusplus
extern "C" {
#endif

/* C11's _Noreturn is C++'s [[noreturn]]. */
#if defined(__cplusplus) && !defined(_Noreturn)
#define _Noreturn [[noreturn]]
#endif

#include <stddef.h>

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

/* Quotient and remainder, from one division: div(7, 2) is {3, 1}. */
typedef struct { int quot, rem; } div_t;
typedef struct { long quot, rem; } ldiv_t;
typedef struct { long long quot, rem; } lldiv_t;
div_t div(int numerator, int denominator);
ldiv_t ldiv(long numerator, long denominator);
lldiv_t lldiv(long long numerator, long long denominator);

/* The most bytes one character can take in the current locale. The
 * only locale is "C", where every character is one byte. */
#define MB_CUR_MAX ((size_t)1)

/* Ends the program. Returning from main does the same. */
_Noreturn void exit(int code);

/* function, to be called when the program exits (by exit or by
 * returning from main) - the last registered first, before the streams
 * are written out. 0, or -1 if there is no memory for it. */
int atexit(void (*function)(void));

/* Memory, asked for while the program runs. Each returns NULL if there
 * is none to give.
 *
 *   malloc    size bytes, holding whatever was there
 *   calloc    count * size bytes, all zero
 *   realloc   the same bytes in a block of a new size - which may be
 *             somewhere else: use only the address returned
 *   free      gives a block back; free(NULL) does nothing */
void *malloc(size_t size);
void *calloc(size_t count, size_t size);
void *realloc(void *block, size_t size);
void free(void *block);
/* Memory on a multiple of `alignment` bytes (a power of two), for a
 * type that needs more than malloc's 16; freed by free. */
void *aligned_alloc(size_t alignment, size_t size);

/* Text into a number: spaces, an optional sign, then digits in the
 * given base (2 to 36; letters are the digits past 9). Base 0 reads
 * the text as C would: 0x in front means sixteen, 0 means eight,
 * otherwise ten.
 *
 * If end is not NULL, *end is left pointing just after the last digit
 * used - at the start of the text if there was no number there at
 * all, which is how to tell "0" from "not a number". A number too big
 * to fit gives the biggest (or, for strtol, the most negative) there
 * is, and sets errno to ERANGE. */
long strtol(const char *text, char **end, int base);
unsigned long strtoul(const char *text, char **end, int base);
/* The same for long long, which here is the same size as long. */
long long strtoll(const char *text, char **end, int base);
unsigned long long strtoull(const char *text, char **end, int base);

/* Text into a floating-point number: spaces, an optional sign, then
 * digits with at most one point, then an exponent (e or E, a sign,
 * digits) if one is there - or inf, infinity, nan, in any case. The
 * nearest number of the type, correctly rounded. Hexadecimal ("0x1p3")
 * is not read. *end and ERANGE as for strtol; ERANGE also when the
 * number is too small and comes out 0. strtold is the nearest double,
 * made a long double. */
double strtod(const char *text, char **end);
float strtof(const char *text, char **end);
long double strtold(const char *text, char **end);
double atof(const char *text);

/* The program's environment: what the program that started it passed
 * (posix_spawn's envp; system and popen pass environ), or - started by
 * the shell, which has none - nothing. What it sets with setenv or
 * putenv, getenv finds, environ lists, and a program it starts gets. */
char *getenv(const char *name);
int setenv(const char *name, const char *value, int overwrite);
int unsetenv(const char *name);
int putenv(char *entry);

/* Runs a command and waits for it; its status, as waitpid's. A plain
 * pipeline - words as sh splits them, pipes (a | b), redirections
 * (< f, > f, >> f, 2> f, 2>&1, >&2) - is started from here, each
 * command's first word as posix_spawnp starts a name: a bare name is
 * the file of that name in the root. Anything more (';', '&&', '$', a
 * wildcard, VAR=x cmd, a builtin such as cd or exit) is run by
 * `/sh -c command` (libc/programs/sh.c); with no /sh: -1, ENOSYS, and
 * a line on standard error. A pipeline's status is its last
 * command's. A program that cannot be started, or a redirection's file
 * that cannot be opened: status 127, as sh's "not found", with a line
 * saying why. Without the right to start programs (`with programs`):
 * -1, EPERM. system(NULL) is 1. */
int system(const char *command);

/* The whole path from the root that path names, if something is there:
 * into resolved (PATH_MAX bytes), or - for NULL - into memory from
 * malloc, for the caller to free. NULL, with errno, if nothing is. */
char *realpath(const char *path, char *resolved);

/* Ends the program at once, with code 134 (what a shell shows for a
 * program killed by SIGABRT): nothing waiting in streams is written. */
_Noreturn void abort(void);

/* Ends the program at once with `code`, as _exit (unistd.h) does:
 * nothing waiting in streams is written, no atexit handler runs. */
_Noreturn void _Exit(int code);

/* Numbers that look random and are not: the same sequence every time
 * for the same srand seed (1 if srand is never called) - the C
 * standard's own example generator. For games and tests, not secrets. */
#define RAND_MAX 32767
int rand(void);
void srand(unsigned seed);

/* The same in base ten, with no way of saying it failed. */
int atoi(const char *text);
long atol(const char *text);

/* The number without its sign. */
int abs(int number);
long labs(long number);
long long llabs(long long number);
long long atoll(const char *text);

/* Sorts count items of size bytes each, starting at base. compare is
 * given the addresses of two items and returns less than 0, 0, or
 * more than 0 as the first belongs before, with, or after the second.
 * Items that compare equal end up in no particular order. */
void qsort(void *base, size_t count, size_t size, int (*compare)(const void *, const void *));

/* Finds an item equal to key among items that are sorted by the same
 * compare, which is given key first and an item second. NULL if there
 * is none. */
void *bsearch(const void *key, const void *base, size_t count, size_t size,
              int (*compare)(const void *, const void *));

#ifdef __cplusplus
}
#endif

#endif
