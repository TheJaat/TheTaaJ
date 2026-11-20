/* stdio.c - printf and its relations: formatted text to a stream.
 *
 * A stream, for now, is no more than a descriptor: stdout is 1 and
 * stderr is 2. Nothing is kept from one call to the next. Inside one
 * call the text is gathered in a small buffer, so that printf("%d: %s")
 * is one write and not five; when the call returns, everything it
 * printed has been written. (Streams that are files, and that keep
 * text until a line or a buffer is full, come later.) */

#include <stdio.h>

#include <errno.h>
#include <string.h>
#include <unistd.h>

#include "format.h"

struct _FILE {
    int descriptor;
};

static FILE standard_output = { STDOUT_FILENO };
static FILE standard_error = { STDERR_FILENO };

FILE *stdout = &standard_output;
FILE *stderr = &standard_error;

/* The text of one call, on its way to a descriptor. */
struct pending {
    int descriptor;
    int failed;
    size_t used;
    char bytes[256];
};

static void flush(struct pending *pending)
{
    if (pending->used > 0 && write(pending->descriptor, pending->bytes, pending->used) < 0)
        pending->failed = 1;
    pending->used = 0;
}

static void into_stream(void *context, const char *bytes, size_t count)
{
    struct pending *pending = context;
    while (count > 0) {
        size_t fits = sizeof pending->bytes - pending->used;
        if (count < fits)
            fits = count;
        memcpy(pending->bytes + pending->used, bytes, fits);
        pending->used += fits;
        bytes += fits;
        count -= fits;
        if (pending->used == sizeof pending->bytes)
            flush(pending);
    }
}

int vfprintf(FILE *stream, const char *format, va_list arguments)
{
    struct pending pending;
    pending.descriptor = stream->descriptor;
    pending.failed = 0;
    pending.used = 0;
    int total = __taaj_format(into_stream, &pending, format, arguments);
    flush(&pending);
    return pending.failed ? EOF : total;
}

int fprintf(FILE *stream, const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int total = vfprintf(stream, format, arguments);
    va_end(arguments);
    return total;
}

int vprintf(const char *format, va_list arguments)
{
    return vfprintf(stdout, format, arguments);
}

int printf(const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int total = vfprintf(stdout, format, arguments);
    va_end(arguments);
    return total;
}

/* What the last failure was, in words, on standard error - after the
 * caller's own words for what it was doing, if it has any. */
void perror(const char *doing)
{
    int reason = errno; /* before anything below can change it */
    if (doing != NULL && doing[0] != '\0')
        fprintf(stderr, "%s: %s\n", doing, strerror(reason));
    else
        fprintf(stderr, "%s\n", strerror(reason));
}

int fputs(const char *string, FILE *stream)
{
    size_t count = strlen(string);
    if (count > 0 && write(stream->descriptor, string, count) < 0)
        return EOF;
    return 0;
}

/* The string and a new line - as one write, so that two programs
 * printing at once cannot come between them. */
int puts(const char *string)
{
    return printf("%s\n", string) < 0 ? EOF : 0;
}

int fputc(int byte, FILE *stream)
{
    unsigned char one = (unsigned char)byte;
    if (write(stream->descriptor, &one, 1) < 0)
        return EOF;
    return one;
}

int putc(int byte, FILE *stream)
{
    return fputc(byte, stream);
}

int putchar(int byte)
{
    return fputc(byte, stdout);
}
