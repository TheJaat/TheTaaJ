/* stdio.c - streams: a descriptor with a buffer in front of it.
 *
 * read and write (libc/src/descriptors.rs) each cost a message to
 * another program and its answer: the file service for a file, the
 * display for the screen. A program that reads a file a byte at a time
 * with read asks the file service once for every byte. A stream is the
 * cure. It holds some memory, and
 *
 *   writing   bytes are copied into the memory, and sent - all of them
 *             in one write - when it is full
 *   reading   one read fills the memory, and bytes are handed out from
 *             it until it is empty
 *
 * So the same memory is used in two ways, and a stream is doing one or
 * the other at a time:
 *
 *   writing    buffer:  [ waiting to be sent | free            ]
 *                        0                  filled            size
 *
 *   reading    buffer:  [ handed out | not yet read | nothing  ]
 *                        0        position        filled      size
 *
 * The price is that the stream and the descriptor under it disagree
 * about where they are. After writing, the file is behind the program
 * (by `filled` bytes); after reading, the descriptor is ahead of it (by
 * `filled - position` bytes). Most of this file is keeping that
 * straight: before the direction changes, before a move (fseek), and
 * before the program ends, the two are brought back together.
 *
 * This half of the library is C, and asks the other half for only what
 * any C program could: open, close, read, write, lseek, unlink, isatty,
 * malloc and free. That is also what lets it be compiled and tested on
 * the host, with those seven calls replaced by ones that count and can
 * be told to fail (libc/c/stdio_test.c). */

#include <stdio.h>

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "format.h"

/* What a stream may do, what it is doing, and what has happened. */
#define CAN_READ   0x001u
#define CAN_WRITE  0x002u
#define READING    0x004u /* the buffer holds bytes read ahead */
#define WRITING    0x008u /* the buffer holds bytes waiting to be sent */
#define AT_END     0x010u /* a read found nothing more: what feof says */
#define FAILED     0x020u /* a read or write failed: what ferror says */
#define BY_LINE    0x040u /* send at the end of every line */
#define ASK_SCREEN 0x080u /* not decided yet: BY_LINE if it is the screen */
#define ALLOCATED  0x100u /* made by fopen: fclose gives the memory back */

struct _FILE {
    int descriptor;
    unsigned flags;
    unsigned char *buffer;
    size_t size;
    size_t position;
    size_t filled;
    /* The byte ungetc put back, or EOF for none. It has a place of its
     * own so that there is always room for one, wherever in the buffer
     * the reading has got to. */
    int pushed;
    /* The buffer the stream came with (setvbuf may point it at
     * another, and back), and how big. */
    unsigned char *own;
    size_t own_size;
    /* The buffer of a stream that keeps nothing: one byte. With that,
     * "not buffered" is not a special case anywhere below - a buffer of
     * one is full the moment anything is in it. */
    unsigned char single;
    /* The next of the streams fopen made: exit must find them all. */
    FILE *following;
};

static unsigned char input_buffer[BUFSIZ];
static unsigned char output_buffer[BUFSIZ];

static FILE standard_input = {
    .descriptor = STDIN_FILENO,
    .flags = CAN_READ,
    .buffer = input_buffer,
    .size = sizeof input_buffer,
    .pushed = EOF,
    .own = input_buffer,
    .own_size = sizeof input_buffer,
};

/* Whether standard output is the screen is not known until the program
 * runs, so the question is asked the first time it is written to. */
static FILE standard_output = {
    .descriptor = STDOUT_FILENO,
    .flags = CAN_WRITE | ASK_SCREEN,
    .buffer = output_buffer,
    .size = sizeof output_buffer,
    .pushed = EOF,
    .own = output_buffer,
    .own_size = sizeof output_buffer,
};

/* Standard error keeps nothing: a complaint that is still in a buffer
 * when the program crashes is a complaint nobody reads. */
static FILE standard_error = {
    .descriptor = STDERR_FILENO,
    .flags = CAN_WRITE,
    .buffer = &standard_error.single,
    .size = 1,
    .pushed = EOF,
};

FILE *stdin = &standard_input;
FILE *stdout = &standard_output;
FILE *stderr = &standard_error;

/* The streams fopen made and fclose has not yet taken back. */
static FILE *opened;

/* ---- between the buffer and the descriptor ---- */

/* Writes all of it, however many writes that takes. */
static int send(FILE *stream, const unsigned char *bytes, size_t count)
{
    while (count > 0) {
        ssize_t written = write(stream->descriptor, bytes, count);
        if (written <= 0) {
            stream->flags |= FAILED;
            return -1;
        }
        bytes += written;
        count -= (size_t)written;
    }
    return 0;
}

/* Sends what is waiting. If that fails it is dropped: keeping it would
 * only fail again, at every call, for ever. */
static int flush_waiting(FILE *stream)
{
    if (!(stream->flags & WRITING) || stream->filled == 0)
        return 0;
    size_t count = stream->filled;
    stream->filled = 0;
    return send(stream, stream->buffer, count);
}

/* How many bytes the descriptor is ahead of the program. */
static size_t unread(const FILE *stream)
{
    if (!(stream->flags & READING))
        return 0;
    return stream->filled - stream->position + (stream->pushed != EOF);
}

/* Forgets what was read ahead, and moves the descriptor back to where
 * the program thinks it is. Something that is not a file - a pipe -
 * cannot go back, and what was read ahead from it is then lost, as it
 * would be on any system. */
static void drop_read_ahead(FILE *stream)
{
    size_t ahead = unread(stream);
    stream->position = 0;
    stream->filled = 0;
    stream->pushed = EOF;
    if (ahead > 0) {
        int reason = errno; /* failing to go back is not the caller's failure */
        lseek(stream->descriptor, -(off_t)ahead, SEEK_CUR);
        errno = reason;
    }
}

static int begin_writing(FILE *stream)
{
    if (!(stream->flags & CAN_WRITE)) {
        errno = EBADF;
        stream->flags |= FAILED;
        return -1;
    }
    if (stream->flags & READING) {
        drop_read_ahead(stream);
        stream->flags &= ~(READING | AT_END);
    }
    if (stream->flags & ASK_SCREEN) {
        /* isatty leaves a reason in errno when the answer is no. That
         * "no" is not a failure of the printf that asked. */
        int reason = errno;
        stream->flags &= ~ASK_SCREEN;
        if (isatty(stream->descriptor))
            stream->flags |= BY_LINE;
        errno = reason;
    }
    stream->flags |= WRITING;
    return 0;
}

static int begin_reading(FILE *stream)
{
    if (!(stream->flags & CAN_READ)) {
        errno = EBADF;
        stream->flags |= FAILED;
        return -1;
    }
    if (stream->flags & WRITING) {
        if (flush_waiting(stream) < 0)
            return -1;
        stream->flags &= ~WRITING;
    }
    stream->flags |= READING;
    return 0;
}

/* One read, into `to`. Returns how many bytes; 0 at the end or on
 * failure, with the flag that says which. */
static size_t fetch(FILE *stream, unsigned char *to, size_t room)
{
    if (stream->flags & AT_END)
        return 0;
    /* A program that prints "name? " and then reads means the question
     * to be seen before the answer is waited for - though it did not
     * end in a new line. */
    if (stream == &standard_input && (standard_output.flags & BY_LINE))
        flush_waiting(&standard_output);
    ssize_t count = read(stream->descriptor, to, room);
    if (count < 0)
        stream->flags |= FAILED;
    else if (count == 0)
        stream->flags |= AT_END;
    return count > 0 ? (size_t)count : 0;
}

/* Fills the empty buffer. -1 if there was nothing to fill it with. */
static int refill(FILE *stream)
{
    size_t count = fetch(stream, stream->buffer, stream->size);
    stream->position = 0;
    stream->filled = count;
    return count > 0 ? 0 : -1;
}

/* Bytes into a stream that is writing. */
static int put(FILE *stream, const void *from, size_t count)
{
    const unsigned char *bytes = from;
    size_t left = count;
    while (left > 0) {
        /* With nothing waiting and at least a buffer's worth to go,
         * copying it into the buffer first would be work for nothing. */
        if (stream->filled == 0 && left >= stream->size)
            return send(stream, bytes, left);
        size_t fits = stream->size - stream->filled;
        if (left < fits)
            fits = left;
        memcpy(stream->buffer + stream->filled, bytes, fits);
        stream->filled += fits;
        bytes += fits;
        left -= fits;
        if (stream->filled == stream->size && flush_waiting(stream) < 0)
            return -1;
    }
    if ((stream->flags & BY_LINE) && memchr(from, '\n', count) != NULL)
        return flush_waiting(stream);
    return 0;
}

/* ---- every stream ---- */

static int flush_all(void)
{
    int result = 0;
    if (flush_waiting(&standard_output) < 0)
        result = EOF;
    for (FILE *stream = opened; stream != NULL; stream = stream->following) {
        if (flush_waiting(stream) < 0)
            result = EOF;
    }
    return result;
}

/* What exit calls (libc/src/lib.rs) before the program ends. */
void __taaj_flush_streams(void)
{
    flush_all();
}

int fflush(FILE *stream)
{
    if (stream == NULL)
        return flush_all();
    if (stream->flags & WRITING)
        return flush_waiting(stream) < 0 ? EOF : 0;
    /* For a stream being read, "flush" is: let the descriptor be where
     * the program is, so that someone else can go on from there. */
    drop_read_ahead(stream);
    return 0;
}

/* ---- opening and closing ---- */

/* A mode as fopen takes it ("r", "w+", "ab") into open's flags and the
 * stream's. -1 if it is not a mode. */
static int parse_mode(const char *mode, unsigned *flags)
{
    int how;
    int both = 0;
    switch (mode[0]) {
    case 'r':
        how = 0;
        break;
    case 'w':
        how = O_CREAT | O_TRUNC;
        break;
    case 'a':
        how = O_CREAT | O_APPEND;
        break;
    default:
        return -1;
    }
    for (const char *more = mode + 1; *more != '\0'; more++) {
        switch (*more) {
        case '+':
            both = 1;
            break;
        case 'x':
            how |= O_EXCL;
            break;
        case 'b':
            break;
        default:
            return -1;
        }
    }
    if (both) {
        *flags = CAN_READ | CAN_WRITE;
        return how | O_RDWR;
    }
    *flags = mode[0] == 'r' ? CAN_READ : CAN_WRITE;
    return how | (mode[0] == 'r' ? O_RDONLY : O_WRONLY);
}

FILE *fopen(const char *path, const char *mode)
{
    unsigned flags;
    int how = parse_mode(mode, &flags);
    if (how < 0) {
        errno = EINVAL;
        return NULL;
    }
    /* The stream and its buffer are one piece of memory: the buffer
     * starts where the stream ends. Asked for before the file is
     * opened, so that a "w" that cannot have a stream does not empty
     * the file on its way to failing. */
    FILE *stream = malloc(sizeof *stream + BUFSIZ);
    if (stream == NULL)
        return NULL; /* malloc has said why */
    int descriptor = open(path, how, 0666);
    if (descriptor < 0) {
        int reason = errno;
        free(stream);
        errno = reason;
        return NULL;
    }
    memset(stream, 0, sizeof *stream);
    stream->descriptor = descriptor;
    stream->flags = flags | ALLOCATED;
    stream->own = (unsigned char *)(stream + 1);
    stream->own_size = BUFSIZ;
    stream->buffer = stream->own;
    stream->size = stream->own_size;
    stream->pushed = EOF;
    stream->following = opened;
    opened = stream;
    return stream;
}

FILE *fdopen(int descriptor, const char *mode)
{
    unsigned flags;
    if (parse_mode(mode, &flags) < 0) {
        errno = EINVAL;
        return NULL;
    }
    FILE *stream = malloc(sizeof *stream + BUFSIZ);
    if (stream == NULL)
        return NULL;
    memset(stream, 0, sizeof *stream);
    stream->descriptor = descriptor;
    stream->flags = flags | ALLOCATED;
    stream->own = (unsigned char *)(stream + 1);
    stream->own_size = BUFSIZ;
    stream->buffer = stream->own;
    stream->size = stream->own_size;
    stream->pushed = EOF;
    stream->following = opened;
    opened = stream;
    return stream;
}

/* What assert says when its condition is false, before it ends the
 * program the way abort does. Here, beside fprintf, to say it with. */
_Noreturn void __taaj_assert_failed(const char *condition, const char *file, int line, const char *function)
{
    fprintf(stderr, "%s:%d: %s: assertion failed: %s\n", file, line, function, condition);
    fflush(stderr);
    abort();
}

int fclose(FILE *stream)
{
    int result = flush_waiting(stream) < 0 ? EOF : 0;
    if (close(stream->descriptor) < 0)
        result = EOF;
    if (stream->flags & ALLOCATED) {
        /* Out of the list: `link` is whichever pointer points at it. */
        FILE **link = &opened;
        while (*link != NULL && *link != stream)
            link = &(*link)->following;
        if (*link == stream)
            *link = stream->following;
        free(stream);
    } else {
        /* One of the three: it stays, able to do nothing. */
        stream->flags = 0;
        stream->descriptor = -1;
        stream->position = 0;
        stream->filled = 0;
    }
    return result;
}

int setvbuf(FILE *stream, char *buffer, int mode, size_t size)
{
    if (mode != _IOFBF && mode != _IOLBF && mode != _IONBF)
        return -1;
    /* Not with bytes in the buffer it has: they would be left behind. */
    if (stream->filled != 0 || stream->pushed != EOF)
        return -1;
    if (mode == _IONBF) {
        stream->buffer = &stream->single;
        stream->size = 1;
    } else if (buffer != NULL && size > 0) {
        stream->buffer = (unsigned char *)buffer;
        stream->size = size;
    } else if (stream->own != NULL) {
        stream->buffer = stream->own;
        stream->size = stream->own_size;
    } else {
        return -1; /* stderr has no buffer of its own to go back to */
    }
    stream->flags &= ~(BY_LINE | ASK_SCREEN);
    if (mode == _IOLBF)
        stream->flags |= BY_LINE;
    return 0;
}

/* ---- reading ---- */

int fgetc(FILE *stream)
{
    if (begin_reading(stream) < 0)
        return EOF;
    if (stream->pushed != EOF) {
        int byte = stream->pushed;
        stream->pushed = EOF;
        return byte;
    }
    if (stream->position == stream->filled && refill(stream) < 0)
        return EOF;
    return stream->buffer[stream->position++];
}

int getc(FILE *stream)
{
    return fgetc(stream);
}

int getchar(void)
{
    return fgetc(stdin);
}

int ungetc(int byte, FILE *stream)
{
    if (byte == EOF || begin_reading(stream) < 0 || stream->pushed != EOF)
        return EOF;
    stream->pushed = (unsigned char)byte;
    stream->flags &= ~AT_END; /* there is something to read again */
    return stream->pushed;
}

char *fgets(char *to, int size, FILE *stream)
{
    if (size <= 0)
        return NULL;
    int kept = 0;
    while (kept < size - 1) {
        int byte = fgetc(stream);
        if (byte == EOF) {
            /* Half a line and then a failure is not a line. */
            if (stream->flags & FAILED)
                return NULL;
            break;
        }
        to[kept++] = (char)byte;
        if (byte == '\n')
            break;
    }
    /* Nothing at all, at the end: that is how a loop over the lines of
     * a file is told to stop. */
    if (kept == 0 && size > 1)
        return NULL;
    to[kept] = '\0';
    return to;
}

/* size times count into *total; false if it does not fit in a size_t
 * (then fread and fwrite do nothing). Plain C rather than clang's
 * __builtin_mul_overflow, so that TCC compiles this file too. */
static int product(size_t size, size_t count, size_t *total)
{
    if (count != 0 && size > SIZE_MAX / count)
        return 0;
    *total = size * count;
    return 1;
}

size_t fread(void *to, size_t size, size_t count, FILE *stream)
{
    size_t total;
    if (!product(size, count, &total) || total == 0)
        return 0;
    if (begin_reading(stream) < 0)
        return 0;
    unsigned char *out = to;
    size_t done = 0;
    if (stream->pushed != EOF) {
        out[done++] = (unsigned char)stream->pushed;
        stream->pushed = EOF;
    }
    while (done < total) {
        size_t have = stream->filled - stream->position;
        if (have > 0) {
            if (have > total - done)
                have = total - done;
            memcpy(out + done, stream->buffer + stream->position, have);
            stream->position += have;
            done += have;
        } else if (total - done >= stream->size) {
            /* More wanted than the buffer holds: straight to where it
             * is going, without stopping in the buffer on the way. */
            size_t got = fetch(stream, out + done, total - done);
            if (got == 0)
                break;
            done += got;
        } else if (refill(stream) < 0) {
            break;
        }
    }
    return done / size;
}

/* ---- writing ---- */

size_t fwrite(const void *from, size_t size, size_t count, FILE *stream)
{
    size_t total;
    if (!product(size, count, &total) || total == 0)
        return 0;
    if (begin_writing(stream) < 0 || put(stream, from, total) < 0)
        return 0;
    return count;
}

int fputc(int byte, FILE *stream)
{
    unsigned char one = (unsigned char)byte;
    if (begin_writing(stream) < 0 || put(stream, &one, 1) < 0)
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

int fputs(const char *string, FILE *stream)
{
    if (begin_writing(stream) < 0 || put(stream, string, strlen(string)) < 0)
        return EOF;
    return 0;
}

/* The string and a new line. */
int puts(const char *string)
{
    if (fputs(string, stdout) == EOF || fputc('\n', stdout) == EOF)
        return EOF;
    return 0;
}

/* Where printf's text goes: a stream, and whether it took it. */
struct target {
    FILE *stream;
    int failed;
};

static void into_stream(void *context, const char *bytes, size_t count)
{
    struct target *target = context;
    if (put(target->stream, bytes, count) < 0)
        target->failed = 1;
}

int vfprintf(FILE *stream, const char *format, va_list arguments)
{
    if (begin_writing(stream) < 0)
        return EOF;
    struct target target = { stream, 0 };
    /* The text arrives in pieces: for "%s: %d\n", the string, then
     * ": ", then the digits, then the new line. A stream that keeps
     * nothing would send each as a write of its own, and another
     * program's text could land between them on the screen. So for
     * the length of this call it is lent a buffer, and what one call
     * printed is one write. */
    unsigned char lent[256];
    unsigned char *its_buffer = stream->buffer;
    size_t its_size = stream->size;
    int lend = its_size < sizeof lent && stream->filled == 0;
    if (lend) {
        stream->buffer = lent;
        stream->size = sizeof lent;
    }
    int total = __taaj_format(into_stream, &target, format, arguments);
    if (lend) {
        if (flush_waiting(stream) < 0)
            target.failed = 1;
        stream->buffer = its_buffer;
        stream->size = its_size;
    }
    return target.failed ? EOF : total;
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

/* ---- where a stream is ---- */

int fseek(FILE *stream, long offset, int whence)
{
    if (stream->flags & WRITING) {
        if (flush_waiting(stream) < 0)
            return -1;
    } else if (whence == SEEK_CUR) {
        /* "From here" is from where the program is, and the descriptor
         * is ahead of that by what was read and not handed out. */
        offset -= (long)unread(stream);
    }
    if (lseek(stream->descriptor, offset, whence) < 0)
        return -1;
    /* Neither reading nor writing now: whichever comes next starts
     * with an empty buffer, at the new place. */
    stream->position = 0;
    stream->filled = 0;
    stream->pushed = EOF;
    stream->flags &= ~(READING | WRITING | AT_END);
    return 0;
}

long ftell(FILE *stream)
{
    off_t place = lseek(stream->descriptor, 0, SEEK_CUR);
    if (place < 0)
        return -1;
    if (stream->flags & WRITING)
        return place + (long)stream->filled;
    return place - (long)unread(stream);
}

void rewind(FILE *stream)
{
    fseek(stream, 0, SEEK_SET);
    stream->flags &= ~FAILED;
}

int feof(FILE *stream)
{
    return (stream->flags & AT_END) != 0;
}

int ferror(FILE *stream)
{
    return (stream->flags & FAILED) != 0;
}

void clearerr(FILE *stream)
{
    stream->flags &= ~(AT_END | FAILED);
}

int fileno(FILE *stream)
{
    return stream->descriptor;
}

int remove(const char *path)
{
    return unlink(path);
}
