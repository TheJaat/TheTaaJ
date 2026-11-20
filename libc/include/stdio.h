/* stdio.h - text out, formatted.
 *
 * So far: the printf family, and the plain putting of strings and
 * bytes, to the two streams every program starts with. Reading, and
 * streams that are files, are not here yet.
 *
 * A format is text with conversions in it, each of the shape
 *
 *     %  [flags]  [width]  [.precision]  [length]  conversion
 *
 *   conversion   d i  a whole number        u  one without a sign
 *                x X  in hex                o  in octal
 *                c    one byte              s  a string
 *                p    an address            %  a percent sign
 *   flags        -  to the left of the width     0  zeros, not spaces
 *                +  a sign always                space  or a space
 *                #  0x before hex, 0 before octal
 *   width        the least room to take; * takes it from an argument
 *   precision    the least digits of a number, the most bytes of a
 *                string; * takes it from an argument
 *   length       hh h l ll z j t: the argument is a char, short, long,
 *                long long, size_t, intmax_t, ptrdiff_t
 *
 * Not here: the floating-point conversions (f e g a), and n. */
#ifndef _STDIO_H
#define _STDIO_H

#include <stdarg.h>
#include <stddef.h>

/* What a function returns when it could not write. */
#define EOF (-1)

/* A stream. What is inside is the library's business. */
typedef struct _FILE FILE;

extern FILE *stdout;
extern FILE *stderr;
#define stdout stdout
#define stderr stderr

/* Each returns how many bytes the text came to, or EOF. The compiler
 * is told which argument is the format, and checks the others against
 * it: printf("%s", 5) does not compile. */
int printf(const char *format, ...) __attribute__((format(printf, 1, 2)));
int fprintf(FILE *stream, const char *format, ...) __attribute__((format(printf, 2, 3)));
int vprintf(const char *format, va_list arguments) __attribute__((format(printf, 1, 0)));
int vfprintf(FILE *stream, const char *format, va_list arguments) __attribute__((format(printf, 2, 0)));

/* Into a buffer of size bytes: never more than that, always ending in
 * a zero (if size is not 0). Returns how long the whole text is - more
 * than size - 1 means it was cut short. */
int snprintf(char *to, size_t size, const char *format, ...) __attribute__((format(printf, 3, 4)));
int vsnprintf(char *to, size_t size, const char *format, va_list arguments) __attribute__((format(printf, 3, 0)));

/* Into a buffer whose size they are not told. Use snprintf. */
int sprintf(char *to, const char *format, ...) __attribute__((format(printf, 2, 3)));
int vsprintf(char *to, const char *format, va_list arguments) __attribute__((format(printf, 2, 0)));

/* A string; puts adds a new line. Not negative, or EOF. */
int fputs(const char *string, FILE *stream);
int puts(const char *string);

/* One byte. Returns it, or EOF. */
int fputc(int byte, FILE *stream);
int putc(int byte, FILE *stream);
int putchar(int byte);

#endif
