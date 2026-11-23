/* stdio.h - streams: reading and writing through a buffer.
 *
 * A stream (a FILE *) is a descriptor with memory in front of it.
 * What a program writes waits in that memory until there is enough of
 * it to be worth sending; what it reads is fetched a buffer at a time
 * and handed out from there. So putc a thousand times is one write,
 * and fgetc a thousand times is one read.
 *
 * Every program starts with three:
 *
 *   stdin    standard input: what the program was given to read (a
 *            file with <, another program with |), or else what is
 *            typed - a line at a time, as each is finished with Enter
 *   stdout   standard output. On the screen, text is sent at the end
 *            of each line; into a file or a pipe, when the buffer is
 *            full
 *   stderr   standard error: nothing waits, every call is sent at once
 *
 * fopen makes more. Whatever still waits is sent when the stream is
 * closed, when fflush is called, and when the program ends by exit or
 * by returning from main - but not if it ends by _exit, or is stopped.
 *
 * Not here: scanf and its family, tmpfile, and wide characters.
 *
 * printf
 * ------
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

#ifdef __cplusplus
extern "C" {
#endif

#include <stdarg.h>
#include <stddef.h>

/* What a function returns when it could not write, or when there is
 * nothing more to read. */
#define EOF (-1)

/* How big a stream's buffer is. */
#define BUFSIZ 4096

/* A stream. What is inside is the library's business. */
typedef struct _FILE FILE;

extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;
#define stdin stdin
#define stdout stdout
#define stderr stderr

/* Opens the file at path as a stream. Returns NULL if it cannot, with
 * the reason in errno. The mode is one of
 *
 *   "r"   to read; the file must be there
 *   "w"   to write: the file is made, or emptied
 *   "a"   to write at its end: the file is made, or kept
 *
 * and after it any of: + (to read and to write), x (with w: fail if
 * the file is already there), b (allowed, and means nothing here).
 *
 * Like open, this fails unless the program was started with the files
 * (`with files`, or `with readonly` to read). */
FILE *fopen(const char *path, const char *mode);

/* A command's output, or input, as a stream: it needs a pipe between
 * two programs, which TheTaaJ does not have yet. popen is NULL, with
 * ENOSYS; pclose -1. */
FILE *popen(const char *command, const char *mode);
int pclose(FILE *stream);

/* A stream for a descriptor that is already open, with a mode as
 * fopen's (which must suit how the descriptor was opened; it is not
 * checked). fclose closes the descriptor too. */
FILE *fdopen(int descriptor, const char *mode);

/* Sends what waits, and gives the stream up. 0, or EOF if what waited
 * could not be written: for a file being written, this is the call
 * whose answer says whether the file is whole. */
int fclose(FILE *stream);

/* Sends what waits in the stream now - or in every stream, for NULL. */
int fflush(FILE *stream);

/* How long text may wait. To be called before the stream is used.
 *   _IOFBF   until the buffer is full
 *   _IOLBF   until the end of a line
 *   _IONBF   not at all
 * buffer is size bytes for the stream to use, or NULL to leave it with
 * its own. Returns 0, or -1. */
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2
int setvbuf(FILE *stream, char *buffer, int mode, size_t size);

/* One byte, as a number from 0 to 255 - or EOF, at the end and on
 * failure (feof and ferror say which). */
int fgetc(FILE *stream);
int getc(FILE *stream);
int getchar(void);

/* Puts a byte back to be read again next. One byte is always taken;
 * a second, before the first is read, may not be. */
int ungetc(int byte, FILE *stream);

/* A line, new line included, into size bytes with a zero after it. A
 * line too long for that is returned in pieces. NULL at the end, when
 * nothing was read. */
char *fgets(char *to, int size, FILE *stream);

/* count items of size bytes each. Returns how many whole items were
 * read or written: fewer than count means the end, or a failure. */
size_t fread(void *to, size_t size, size_t count, FILE *stream);
size_t fwrite(const void *from, size_t size, size_t count, FILE *stream);

/* Moves where the next read or write happens, as lseek does, and asks
 * where that is. Not for stdin, stdout or stderr when they are not
 * files. */
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
int fseek(FILE *stream, long offset, int whence);
long ftell(FILE *stream);
void rewind(FILE *stream);

/* Whether a read has met the end; whether anything has failed. Both
 * stay true until clearerr (or, for the end, a move). */
int feof(FILE *stream);
int ferror(FILE *stream);
void clearerr(FILE *stream);

/* The descriptor underneath. */
int fileno(FILE *stream);

/* Removes a file. 0, or -1. */
int remove(const char *path);

/* Gives a file or a directory a new name or place. Nothing is
 * replaced: if `to` is there already, this fails with EEXIST. */
int rename(const char *from, const char *to);

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
/* The same into memory from malloc, as big as it needs to be: *to is
 * set to it, for the program to free. Returns the length, or -1 (and
 * *to is then not set) if there was no memory. (format.c) */
int asprintf(char **to, const char *format, ...) __attribute__((format(printf, 2, 3)));
int vasprintf(char **to, const char *format, va_list arguments) __attribute__((format(printf, 2, 0)));

/* Numbers and words read out of a string, by a format like printf's;
 * returns how many were stored, or EOF if the string ended before the
 * first. Only from strings so far. (scan.c) */
int sscanf(const char *text, const char *format, ...) __attribute__((format(scanf, 2, 3)));
int vsscanf(const char *text, const char *format, va_list arguments) __attribute__((format(scanf, 2, 0)));

/* Says on standard error why the last call failed: "doing: reason". */
void perror(const char *doing);

/* A string; puts adds a new line. Not negative, or EOF. */
int fputs(const char *string, FILE *stream);
int puts(const char *string);

/* One byte. Returns it, or EOF. */
int fputc(int byte, FILE *stream);
int putc(int byte, FILE *stream);
int putchar(int byte);

#ifdef __cplusplus
}
#endif

#endif
