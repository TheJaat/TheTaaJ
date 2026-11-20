/* stdio_test.c - the library's streams, on this machine.
 *
 * Runs on the host, not in TheTaaJ. stdio.c is compiled for the host
 * exactly as it is for TheTaaJ, and then every name in the object file
 * is changed (objcopy, with the list in host_names): what it defines
 * gets "taaj_" in front - taaj_fopen, taaj_fgets - so that it can live
 * in one program with the host's own stdio; and so do the seven system
 * calls it makes - read becomes taaj_read - which are the functions
 * below. They pass each call on to the host's, and on the way they
 *
 *   count      "a thousand fputc is one write" is then something to
 *              assert, not to believe
 *   shorten    a read or write may do less than it was asked to; here
 *              it can be made to, every time
 *   fail       a disk that stops working cannot be arranged in a
 *              test inside TheTaaJ; here it is one variable
 *
 * and descriptors 0, 1 and 2 never reach the host at all: 1 and 2 are
 * kept in memory to be looked at, and 0 reads from a string.
 *
 * Where the host's stdio does the same thing, the two are given the
 * same file and the same calls and must agree.
 *
 * Built and run by `make libc-test`. */

#define _DEFAULT_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* ---- the library under test, by its changed names ---- */

typedef struct taaj_FILE taaj_FILE;
extern taaj_FILE *taaj_stdin, *taaj_stdout, *taaj_stderr;
extern int taaj_errno;

taaj_FILE *taaj_fopen(const char *path, const char *mode);
int taaj_fclose(taaj_FILE *stream);
int taaj_fflush(taaj_FILE *stream);
int taaj_setvbuf(taaj_FILE *stream, char *buffer, int mode, size_t size);
int taaj_fgetc(taaj_FILE *stream);
int taaj_getchar(void);
int taaj_ungetc(int byte, taaj_FILE *stream);
char *taaj_fgets(char *to, int size, taaj_FILE *stream);
size_t taaj_fread(void *to, size_t size, size_t count, taaj_FILE *stream);
size_t taaj_fwrite(const void *from, size_t size, size_t count, taaj_FILE *stream);
int taaj_fputc(int byte, taaj_FILE *stream);
int taaj_fputs(const char *string, taaj_FILE *stream);
int taaj_puts(const char *string);
int taaj_printf(const char *format, ...);
int taaj_fprintf(taaj_FILE *stream, const char *format, ...);
void taaj_perror(const char *doing);
int taaj_fseek(taaj_FILE *stream, long offset, int whence);
long taaj_ftell(taaj_FILE *stream);
void taaj_rewind(taaj_FILE *stream);
int taaj_feof(taaj_FILE *stream);
int taaj_ferror(taaj_FILE *stream);
void taaj_clearerr(taaj_FILE *stream);
int taaj_fileno(taaj_FILE *stream);
int taaj_remove(const char *path);
void __taaj_flush_streams(void);

/* The library's own BUFSIZ (libc/include/stdio.h), which the host's
 * need not equal. */
#define TAAJ_BUFSIZ 4096

/* ---- the system calls it makes ---- */

int taaj_errno;

static int reads, writes;
static size_t read_limit, write_limit; /* 0: no limit */
static int fail_reads, fail_writes;

/* What was written to descriptors 1 and 2, and in how many writes. */
static char shown[2][8192];
static size_t shown_used[2];
static int shown_writes[2];

/* What descriptor 0 gives, and how much of standard output had been
 * shown the last time it was read. */
static const char *typed = "";
static size_t shown_at_read;

long taaj_read(int fd, void *to, size_t room)
{
    reads++;
    if (fail_reads) {
        taaj_errno = EIO;
        return -1;
    }
    if (read_limit != 0 && room > read_limit)
        room = read_limit;
    if (fd == 0) {
        size_t count = strlen(typed);
        if (count > room)
            count = room;
        memcpy(to, typed, count);
        typed += count;
        shown_at_read = shown_used[0];
        return (long)count;
    }
    long count = read(fd, to, room);
    if (count < 0)
        taaj_errno = errno;
    return count;
}

long taaj_write(int fd, const void *from, size_t count)
{
    writes++;
    if (fail_writes) {
        taaj_errno = EIO;
        return -1;
    }
    if (write_limit != 0 && count > write_limit)
        count = write_limit;
    if (fd == 1 || fd == 2) {
        int which = fd - 1;
        if (shown_used[which] + count > sizeof shown[which])
            abort();
        memcpy(shown[which] + shown_used[which], from, count);
        shown_used[which] += count;
        shown_writes[which]++;
        return (long)count;
    }
    long written = write(fd, from, count);
    if (written < 0)
        taaj_errno = errno;
    return written;
}

long taaj_lseek(int fd, long offset, int whence)
{
    /* The three never reach the host - and must not here either: what
     * the host's descriptor 1 is depends on who ran the test (a
     * terminal cannot be moved about in, a file a log is being written
     * to can), and the test must not. */
    if (fd == 0 || fd == 1 || fd == 2) {
        taaj_errno = ESPIPE;
        return -1;
    }
    long place = lseek(fd, offset, whence);
    if (place < 0)
        taaj_errno = errno;
    return place;
}

int taaj_open(const char *path, int flags, ...)
{
    int fd = open(path, flags, 0644);
    if (fd < 0)
        taaj_errno = errno;
    return fd;
}

int taaj_close(int fd)
{
    if (fd == 0 || fd == 1 || fd == 2)
        return 0; /* the test's own, which it still needs */
    int result = close(fd);
    if (result < 0)
        taaj_errno = errno;
    return result;
}

int taaj_unlink(const char *path)
{
    int result = unlink(path);
    if (result < 0)
        taaj_errno = errno;
    return result;
}

/* Says what it is told to - and always leaves ENOTTY behind, so that
 * the test sees whether printf lets that through. */
static int is_screen;

int taaj_isatty(int fd)
{
    (void)fd;
    taaj_errno = ENOTTY;
    return is_screen;
}

/* ---- checking ---- */

static int checks;
static int failures;

#define CHECK(condition, what)                                \
    do {                                                      \
        checks++;                                             \
        if (!(condition)) {                                   \
            failures++;                                       \
            printf("line %d: not so: %s\n", __LINE__, what);  \
        }                                                     \
    } while (0)

/* A file's contents by the host's own means; the caller frees. */
static char *contents(const char *path, size_t *size)
{
    FILE *file = fopen(path, "rb");
    if (file == NULL) {
        *size = 0;
        return NULL;
    }
    char *bytes = malloc(1 << 20);
    *size = fread(bytes, 1, 1 << 20, file);
    fclose(file);
    return bytes;
}

static int holds(const char *path, const void *wanted, size_t size)
{
    size_t found;
    char *bytes = contents(path, &found);
    int same = bytes != NULL && found == size && memcmp(bytes, wanted, size) == 0;
    free(bytes);
    return same;
}

static int holds_text(const char *path, const char *wanted)
{
    return holds(path, wanted, strlen(wanted));
}

static void make(const char *path, const void *bytes, size_t size)
{
    FILE *file = fopen(path, "wb");
    if (file == NULL || fwrite(bytes, 1, size, file) != size || fclose(file) != 0) {
        printf("cannot make %s\n", path);
        exit(2);
    }
}

static long size_of(const char *path)
{
    struct stat status;
    return stat(path, &status) == 0 ? (long)status.st_size : -1;
}

/* Bytes with no pattern a buffer boundary could hide behind. */
static void fill(unsigned char *bytes, size_t size)
{
    uint32_t state = 12345;
    for (size_t at = 0; at < size; at++) {
        state = state * 1664525u + 1013904223u;
        bytes[at] = (unsigned char)(state >> 24);
    }
}

static int shown_is(int which, const char *wanted)
{
    return shown_used[which] == strlen(wanted) && memcmp(shown[which], wanted, shown_used[which]) == 0;
}

static void forget_shown(void)
{
    shown_used[0] = shown_used[1] = 0;
    shown_writes[0] = shown_writes[1] = 0;
}

/* ---- the same calls, to the host's stdio and to ours ---- */

#define HOST(name) name
#define OURS(name) taaj_##name

/* Reads the file in lines with a buffer of `room`, noting for each
 * call how long the piece was and what it began and ended with. */
#define LINES_SCRIPT(F, T, path, room, out, count)                    \
    do {                                                              \
        T *file = F(fopen)(path, "r");                                \
        char line[512];                                               \
        count = 0;                                                    \
        while (F(fgets)(line, room, file) != NULL && count < 4000) {  \
            size_t length = strlen(line);                             \
            out[count++] = (long)length;                              \
            out[count++] = line[0];                                   \
            out[count++] = line[length - 1];                          \
        }                                                             \
        out[count++] = F(feof)(file);                                 \
        out[count++] = F(ferror)(file);                               \
        F(fclose)(file);                                              \
    } while (0)

/* Moves about a file open for both, reading and writing, and notes
 * where it is and what it read at every step. (An fseek stands
 * between reading and writing each time: C asks for one, and the
 * host's stdio is entitled to need it.) */
#define SEEK_SCRIPT(F, T, path, out, count)                           \
    do {                                                              \
        T *file = F(fopen)(path, "r+");                               \
        char piece[64];                                               \
        count = 0;                                                    \
        out[count++] = F(ftell)(file);                                \
        out[count++] = (long)F(fread)(piece, 1, 10, file);            \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fseek)(file, 5, SEEK_CUR);                   \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fgetc)(file);                                \
        out[count++] = F(fseek)(file, 0, SEEK_CUR);                   \
        out[count++] = (long)F(fwrite)("XYZ", 1, 3, file);            \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fseek)(file, -4, SEEK_END);                  \
        out[count++] = (long)F(fread)(piece, 1, 64, file);            \
        out[count++] = piece[0];                                      \
        out[count++] = F(feof)(file);                                 \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fseek)(file, 5000, SEEK_SET);                \
        out[count++] = F(feof)(file);                                 \
        out[count++] = F(fgetc)(file);                                \
        out[count++] = F(ungetc)('q', file);                          \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fgetc)(file);                                \
        out[count++] = F(fgetc)(file);                                \
        out[count++] = F(fseek)(file, 0, SEEK_END);                   \
        out[count++] = F(fputc)('!', file);                           \
        out[count++] = F(ftell)(file);                                \
        F(rewind)(file);                                              \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fgetc)(file);                                \
        out[count++] = F(fseek)(file, -1, SEEK_SET);                  \
        out[count++] = F(ftell)(file);                                \
        out[count++] = F(fclose)(file);                               \
    } while (0)

int main(void)
{
    /* A directory of its own to make files in, wherever this machine
     * keeps such things. */
    const char *temporary = getenv("TMPDIR");
    char directory[512];
    snprintf(directory, sizeof directory, "%s/taaj-stdio-XXXXXX", temporary != NULL ? temporary : "/tmp");
    if (mkdtemp(directory) == NULL || chdir(directory) != 0) {
        perror("a directory to work in");
        return 2;
    }

    static unsigned char pattern[20000];
    fill(pattern, sizeof pattern);

    /* Writing waits in the buffer. */
    {
        taaj_FILE *file = taaj_fopen("thousand", "w");
        CHECK(file != NULL, "a file opens for writing");
        writes = 0;
        for (int n = 0; n < 1000; n++)
            taaj_fputc(pattern[n], file);
        CHECK(writes == 0, "a thousand fputc: nothing written yet");
        CHECK(size_of("thousand") == 0, "and the file is still empty");
        CHECK(taaj_ftell(file) == 1000, "though ftell counts what is waiting");
        CHECK(taaj_fclose(file) == 0, "fclose succeeds");
        CHECK(writes == 1, "and all thousand went in one write");
        CHECK(holds("thousand", pattern, 1000), "the file holds the thousand bytes");
    }

    /* Reading is fetched ahead. */
    {
        taaj_FILE *file = taaj_fopen("thousand", "r");
        reads = 0;
        int same = 1;
        for (int n = 0; n < 1000; n++)
            same &= taaj_fgetc(file) == pattern[n];
        CHECK(same, "a thousand fgetc give the thousand bytes");
        CHECK(reads == 1, "from one read");
        CHECK(!taaj_feof(file), "having read the last byte is not yet the end");
        CHECK(taaj_fgetc(file) == EOF && taaj_feof(file) && !taaj_ferror(file), "the next fgetc is: EOF, the end, no error");
        reads = 0;
        CHECK(taaj_fgetc(file) == EOF && reads == 0, "at the end, it does not ask again");
        taaj_clearerr(file);
        CHECK(!taaj_feof(file) && taaj_fgetc(file) == EOF && reads == 1, "until clearerr");
        taaj_fclose(file);
    }

    /* fflush on a stream being read puts the descriptor where the
     * program is. */
    {
        taaj_FILE *file = taaj_fopen("thousand", "r");
        taaj_fgetc(file);
        CHECK(lseek(taaj_fileno(file), 0, SEEK_CUR) == 1000, "after one fgetc the descriptor is at the end of what was fetched");
        CHECK(taaj_ftell(file) == 1, "and ftell says 1");
        CHECK(taaj_fflush(file) == 0 && lseek(taaj_fileno(file), 0, SEEK_CUR) == 1, "fflush brings the descriptor back to 1");
        CHECK(taaj_fgetc(file) == pattern[1], "and reading goes on from there");
        taaj_fclose(file);
    }

    /* Lines: against the host, with room for the whole line and
     * without, and with reads that come in small pieces. */
    {
        FILE *file = fopen("lines", "w");
        fprintf(file, "first\n\nthird line, after an empty one\n");
        for (int n = 0; n < 300; n++)
            fputc('a' + n % 26, file);
        fprintf(file, "\nshort\n");
        for (int n = 0; n < 200; n++)
            fprintf(file, "line %d of two hundred\n", n);
        fprintf(file, "the last line has no end");
        fclose(file);

        static long theirs[4100], ours[4100];
        int their_count, our_count;
        int rooms[] = { 512, 64, 7, 2 };
        for (int limit = 0; limit <= 5; limit += 5) {
            read_limit = (size_t)limit;
            for (size_t n = 0; n < sizeof rooms / sizeof rooms[0]; n++) {
                LINES_SCRIPT(HOST, FILE, "lines", rooms[n], theirs, their_count);
                LINES_SCRIPT(OURS, taaj_FILE, "lines", rooms[n], ours, our_count);
                CHECK(their_count == our_count && memcmp(theirs, ours, sizeof theirs[0] * (size_t)their_count) == 0,
                      "fgets gives the same pieces as the host's");
            }
        }
        read_limit = 0;

        char one[1] = { 'x' };
        taaj_FILE *lines = taaj_fopen("lines", "r");
        CHECK(taaj_fgets(one, 0, lines) == NULL, "fgets into no room at all: NULL");
        CHECK(taaj_fgets(one, 1, lines) == one && one[0] == '\0', "fgets into one byte: an empty string");
        CHECK(taaj_fgetc(lines) == 'f', "and neither read anything");
        taaj_fclose(lines);
    }

    /* Blocks bigger than the buffer, in items. */
    {
        taaj_FILE *file = taaj_fopen("big", "w");
        writes = 0;
        CHECK(taaj_fwrite(pattern, 8, 2500, file) == 2500, "fwrite of 2500 items of 8 says 2500");
        CHECK(writes == 1, "more than a buffer goes straight out, in one write");
        CHECK(taaj_fwrite(pattern, 1, 100, file) == 100 && writes == 1, "and a little more waits");
        CHECK(taaj_fclose(file) == 0 && writes == 2, "until fclose");

        static unsigned char back[20200];
        file = taaj_fopen("big", "r");
        CHECK(taaj_fread(back, 3, 6000, file) == 6000, "fread of 6000 items of 3 says 6000");
        CHECK(memcmp(back, pattern, 18000) == 0, "and they are the bytes written");
        CHECK(taaj_fread(back, 7, 1000, file) == 300, "asking for more than is left gives the whole items there are");
        CHECK(taaj_feof(file) && !taaj_ferror(file), "and that was the end, not a failure");
        CHECK(memcmp(back, pattern + 18000, 2000) == 0 && memcmp(back + 2000, pattern, 100) == 0, "with the right bytes");
        taaj_fclose(file);

        CHECK(taaj_fwrite(pattern, SIZE_MAX, 2, taaj_stderr) == 0, "a size times a count that overflows writes nothing");
        CHECK(taaj_fread(back, 0, 5, taaj_stdin) == 0, "items of no size: none");
    }

    /* Reads and writes that do less than they were asked to. */
    {
        static unsigned char back[20000];
        read_limit = 10;
        taaj_FILE *file = taaj_fopen("big", "r");
        CHECK(taaj_fread(back, 1, 20000, file) == 20000 && memcmp(back, pattern, 20000) == 0,
              "reads of 10 bytes at most: fread still fills what it was given");
        taaj_fclose(file);
        read_limit = 0;

        write_limit = 7;
        file = taaj_fopen("pieces", "w");
        CHECK(taaj_fwrite(pattern, 1, 9000, file) == 9000, "writes of 7 bytes at most: fwrite still says all");
        for (int n = 0; n < 500; n++)
            taaj_fputc(pattern[9000 + n], file);
        CHECK(taaj_fclose(file) == 0, "and fclose succeeds");
        write_limit = 0;
        CHECK(holds("pieces", pattern, 9500), "every byte arrived, in order");
    }

    /* A disk that stops. */
    {
        taaj_FILE *file = taaj_fopen("failing", "w");
        fail_writes = 1;
        taaj_errno = 0;
        CHECK(taaj_fputc('a', file) == 'a' && !taaj_ferror(file), "a failing disk is not noticed by fputc: the byte only waits");
        CHECK(taaj_fflush(file) == EOF, "fflush is where it is noticed");
        CHECK(taaj_ferror(file) && taaj_errno == EIO, "with ferror set and the reason in errno");
        writes = 0;
        CHECK(taaj_fflush(file) == 0 && writes == 0, "what could not be written is dropped, not tried for ever");
        taaj_clearerr(file);
        CHECK(!taaj_ferror(file), "clearerr clears it");
        taaj_fputs("lost", file);
        CHECK(taaj_fclose(file) == EOF, "fclose says when the last of the file did not get there");
        CHECK(taaj_fwrite(pattern, 1, 9000, taaj_stderr) == 0, "fwrite straight to a failing descriptor: 0 items");
        taaj_clearerr(taaj_stderr);
        fail_writes = 0;

        file = taaj_fopen("thousand", "r");
        fail_reads = 1;
        CHECK(taaj_fgetc(file) == EOF && taaj_ferror(file) && !taaj_feof(file), "a failing read: EOF, an error, not the end");
        char line[16];
        CHECK(taaj_fgets(line, sizeof line, file) == NULL, "fgets: NULL");
        CHECK(taaj_fread(line, 1, sizeof line, file) == 0, "fread: 0");
        fail_reads = 0;
        CHECK(taaj_fgetc(file) == pattern[0], "and when the disk is back, reading goes on");
        taaj_fclose(file);
    }

    /* Moving, and both directions on one stream: against the host. */
    {
        make("seek-theirs", pattern, 3000);
        make("seek-ours", pattern, 3000);
        long theirs[40], ours[40];
        int their_count, our_count;
        SEEK_SCRIPT(HOST, FILE, "seek-theirs", theirs, their_count);
        SEEK_SCRIPT(OURS, taaj_FILE, "seek-ours", ours, our_count);
        int same = their_count == our_count;
        for (int n = 0; same && n < their_count; n++) {
            if (theirs[n] != ours[n]) {
                printf("step %d of the seek script: host %ld, ours %ld\n", n, theirs[n], ours[n]);
                same = 0;
            }
        }
        CHECK(same, "fseek, ftell, rewind, ungetc and the rest answer as the host's do");
        size_t size;
        char *bytes = contents("seek-theirs", &size);
        CHECK(holds("seek-ours", bytes, size), "and leave the same file behind");
        free(bytes);
    }

    /* Changing direction without an fseek: more than C promises, and
     * what this library does. */
    {
        make("turn", "abcdefgh", 8);
        taaj_FILE *file = taaj_fopen("turn", "r+");
        CHECK(taaj_fgetc(file) == 'a' && taaj_fgetc(file) == 'b', "two bytes read");
        CHECK(taaj_fputc('X', file) == 'X', "then one written, with no fseek between");
        CHECK(taaj_fgetc(file) == 'd', "and reading again gives the byte after it");
        CHECK(taaj_fclose(file) == 0 && holds_text("turn", "abXdefgh"), "the written byte went where the reading had got to");
    }

    /* ungetc. */
    {
        make("push", "xyz", 3);
        taaj_FILE *file = taaj_fopen("push", "r");
        CHECK(taaj_ungetc('a', file) == 'a', "ungetc before anything is read");
        CHECK(taaj_ungetc('b', file) == EOF, "a second, with the first not read: no");
        CHECK(taaj_fgetc(file) == 'a' && taaj_fgetc(file) == 'x', "the byte put back comes first, then the file");
        CHECK(taaj_ungetc(EOF, file) == EOF, "EOF cannot be put back");
        taaj_ungetc(200, file);
        char three[3];
        CHECK(taaj_fread(three, 1, 3, file) == 3 && (unsigned char)three[0] == 200 && three[1] == 'y' && three[2] == 'z',
              "fread takes it too");
        CHECK(taaj_fgetc(file) == EOF && taaj_feof(file), "the end");
        CHECK(taaj_ungetc('!', file) == '!' && !taaj_feof(file) && taaj_fgetc(file) == '!', "ungetc at the end: there is something to read again");
        taaj_fclose(file);
    }

    /* Adding at the end. */
    {
        make("log", "one\n", 4);
        taaj_FILE *file = taaj_fopen("log", "a");
        taaj_fputs("two\n", file);
        CHECK(taaj_fclose(file) == 0 && holds_text("log", "one\ntwo\n"), "\"a\" adds after what was there");
        file = taaj_fopen("log", "a+");
        char line[16];
        CHECK(taaj_fgets(line, sizeof line, file) != NULL && strcmp(line, "one\n") == 0, "\"a+\" reads from the start");
        taaj_fputs("three\n", file);
        CHECK(taaj_fclose(file) == 0 && holds_text("log", "one\ntwo\nthree\n"), "and still writes at the end");
        file = taaj_fopen("newlog", "a");
        CHECK(file != NULL && taaj_fclose(file) == 0 && size_of("newlog") == 0, "\"a\" makes a file that is not there");
    }

    /* Modes, and what a stream was not opened for. */
    {
        taaj_errno = 0;
        CHECK(taaj_fopen("log", "q") == NULL && taaj_errno == EINVAL, "a mode that is not one: EINVAL");
        CHECK(taaj_fopen("log", "rz") == NULL && taaj_errno == EINVAL, "nor with a letter that means nothing");
        CHECK(taaj_fopen("nothing-here", "r") == NULL && taaj_errno == ENOENT, "\"r\" does not make a file: ENOENT");
        CHECK(taaj_fopen("log", "wx") == NULL && taaj_errno == EEXIST, "\"wx\" will not take a file that is there");
        CHECK(holds_text("log", "one\ntwo\nthree\n"), "and did not empty it on the way");
        taaj_FILE *file = taaj_fopen("fresh", "wxb");
        CHECK(file != NULL && taaj_fclose(file) == 0, "\"wxb\" makes one that is not");

        file = taaj_fopen("log", "r");
        taaj_errno = 0;
        CHECK(taaj_fputc('x', file) == EOF && taaj_ferror(file) && taaj_errno == EBADF, "writing to a stream opened to read: EBADF");
        CHECK(taaj_fprintf(file, "x") == EOF && taaj_fputs("x", file) == EOF && taaj_fwrite("x", 1, 1, file) == 0, "by any of the ways");
        taaj_fclose(file);
        CHECK(holds_text("log", "one\ntwo\nthree\n"), "and the file is as it was");
        file = taaj_fopen("fresh", "w");
        taaj_errno = 0;
        CHECK(taaj_fgetc(file) == EOF && taaj_ferror(file) && taaj_errno == EBADF, "reading from a stream opened to write: EBADF");
        taaj_fclose(file);

        file = taaj_fopen("fresh", "w+");
        taaj_fputs("written", file);
        taaj_rewind(file);
        char line[16];
        CHECK(taaj_fgets(line, sizeof line, file) != NULL && strcmp(line, "written") == 0, "\"w+\" reads back what it wrote");
        taaj_fclose(file);
    }

    /* How long text waits: setvbuf. */
    {
        taaj_FILE *file = taaj_fopen("modes", "w");
        CHECK(taaj_setvbuf(file, NULL, _IONBF, 0) == 0, "setvbuf: keep nothing");
        writes = 0;
        taaj_fputc('a', file);
        taaj_fputc('b', file);
        CHECK(writes == 2, "then every fputc is a write");
        writes = 0;
        CHECK(taaj_fprintf(file, "%d and %s\n", 12, "text") == 12 && writes == 1, "but one fprintf is still one write");

        char small[16];
        CHECK(taaj_setvbuf(file, small, _IOFBF, sizeof small) == 0, "setvbuf: a buffer of 16");
        writes = 0;
        for (int n = 0; n < 15; n++)
            taaj_fputc('.', file);
        CHECK(writes == 0, "15 bytes wait");
        taaj_fputc('.', file);
        CHECK(writes == 1, "the 16th sends them");
        taaj_fputc('c', file);
        CHECK(taaj_setvbuf(file, NULL, _IOLBF, 0) == -1, "setvbuf with a byte waiting: no");
        taaj_fflush(file);

        CHECK(taaj_setvbuf(file, NULL, _IOLBF, 0) == 0, "setvbuf: by the line, in its own buffer again");
        writes = 0;
        taaj_fputs("half a line", file);
        CHECK(writes == 0, "half a line waits");
        taaj_fputs(", and its end\nand more", file);
        CHECK(writes == 1, "the end of the line sends it");
        CHECK(taaj_setvbuf(file, NULL, 99, 0) == -1, "a mode that is not one: no");
        taaj_fclose(file);
        CHECK(holds_text("modes", "ab12 and text\n................chalf a line, and its end\nand more"), "and all of it is in the file, in order");
        CHECK(taaj_setvbuf(taaj_stderr, NULL, _IOFBF, 0) == -1, "stderr has no buffer of its own to be given");
    }

    /* Standard output on the screen: a line at a time. */
    {
        is_screen = 1;
        taaj_errno = 0;
        forget_shown();
        CHECK(taaj_printf("no end of line yet") == 18 && shown_writes[0] == 0, "on the screen, printf without a new line waits");
        CHECK(taaj_errno == 0, "and asking whether it is the screen left errno alone");
        taaj_printf(", %d", 42);
        taaj_printf(" - done\n");
        CHECK(shown_writes[0] == 1 && shown_is(0, "no end of line yet, 42 - done\n"), "the new line sends the whole line, in one write");
        forget_shown();
        CHECK(taaj_puts("a line from puts") == 0 && shown_writes[0] == 1 && shown_is(0, "a line from puts\n"), "puts is one write");
        forget_shown();
        taaj_printf("two\nlines\nand a bit");
        CHECK(shown_is(0, "two\nlines\nand a bit"), "text with new lines inside is sent whole");
        taaj_printf("\n");

        /* A question, then reading the answer. */
        forget_shown();
        typed = "yes\n";
        taaj_printf("go on? ");
        CHECK(shown_writes[0] == 0, "a question without a new line waits");
        CHECK(taaj_getchar() == 'y', "getchar reads the answer");
        CHECK(shown_at_read == 7 && shown_is(0, "go on? "), "and the question was shown before standard input was read");
        char rest[8];
        CHECK(taaj_fgets(rest, sizeof rest, taaj_stdin) != NULL && strcmp(rest, "es\n") == 0, "fgets takes the rest of the line");
        CHECK(taaj_getchar() == EOF && taaj_feof(taaj_stdin), "and then standard input is at its end");
        CHECK(taaj_fseek(taaj_stdout, 0, SEEK_SET) == -1 && taaj_ftell(taaj_stdin) == -1, "neither has a place to move to or ask about");
    }

    /* Standard output into a file or a pipe: a buffer at a time, and
     * at exit. */
    {
        CHECK(taaj_setvbuf(taaj_stdout, NULL, _IOFBF, 0) == 0, "standard output, as it is when not the screen");
        forget_shown();
        for (int n = 0; n < 100; n++)
            taaj_printf("line %d\n", n);
        CHECK(shown_writes[0] == 0, "a hundred short lines: nothing sent");
        taaj_FILE *first = taaj_fopen("exit-1", "w");
        taaj_FILE *second = taaj_fopen("exit-2", "w");
        taaj_FILE *third = taaj_fopen("exit-3", "w");
        taaj_fputs("first", first);
        taaj_fputs("second", second);
        taaj_fputs("third", third);
        taaj_fclose(second); /* from the middle of the list */
        __taaj_flush_streams();
        CHECK(shown_writes[0] == 1 && shown_used[0] == 790, "what exit calls sends them, in one write");
        CHECK(holds_text("exit-1", "first") && holds_text("exit-3", "third"), "and what waited in every open file");
        CHECK(holds_text("exit-2", "second"), "the one closed before had its own already");
        taaj_fputs(" more", first);
        CHECK(taaj_fflush(NULL) == 0 && holds_text("exit-1", "first more"), "fflush(NULL) does the same");
        taaj_fclose(first);
        taaj_fclose(third);

        forget_shown();
        static char long_text[TAAJ_BUFSIZ + 500];
        memset(long_text, 'w', sizeof long_text - 1);
        taaj_printf("%s", long_text);
        CHECK(shown_writes[0] == 1 && shown_used[0] == sizeof long_text - 1,
              "a string longer than the buffer, with nothing waiting, goes straight out");
        forget_shown();
        taaj_printf("ten bytes.%s", long_text);
        CHECK(shown_writes[0] == 1 && shown_used[0] == TAAJ_BUFSIZ, "with ten bytes waiting, a buffer's worth is sent");
        taaj_fflush(taaj_stdout);
        CHECK(shown_writes[0] == 2 && shown_used[0] == sizeof long_text - 1 + 10, "and fflush sends the rest");
    }

    /* Standard error keeps nothing. */
    {
        forget_shown();
        CHECK(taaj_fprintf(taaj_stderr, "%s: %d of %d\n", "error", 3, 4) == 14, "fprintf to stderr returns the length");
        CHECK(shown_writes[1] == 1 && shown_is(1, "error: 3 of 4\n"), "and is one write, at once");
        taaj_fputs("no new line", taaj_stderr);
        CHECK(shown_writes[1] == 2, "fputs without a new line is sent at once too");
        forget_shown();
        static char long_text[700];
        memset(long_text, 'e', sizeof long_text - 1);
        CHECK(taaj_fprintf(taaj_stderr, "[%s]", long_text) == 701 && shown_used[1] == 701, "a long complaint arrives whole");
        forget_shown();
        taaj_errno = ENOENT;
        taaj_perror("opening it");
        char wanted[128];
        snprintf(wanted, sizeof wanted, "opening it: %s\n", strerror(ENOENT));
        CHECK(shown_writes[1] == 1 && shown_is(1, wanted), "perror: what was being done, and why it failed");
    }

    /* remove, and closing one of the three. */
    {
        CHECK(taaj_remove("fresh") == 0 && size_of("fresh") == -1, "remove removes");
        taaj_errno = 0;
        CHECK(taaj_remove("fresh") == -1 && taaj_errno == ENOENT, "and says when there was nothing to");
        taaj_printf("last words");
        forget_shown();
        CHECK(taaj_fclose(taaj_stdout) == 0 && shown_is(0, "last words"), "fclose(stdout) sends what waited");
        taaj_errno = 0;
        CHECK(taaj_printf("after") == EOF && taaj_errno == EBADF, "and printf after it fails");
    }

    /* Clearing up: only if all is well, so that a failure leaves the
     * files to be looked at. */
    if (failures != 0) {
        printf("stdio: %d of %d checks FAILED (files left in %s)\n", failures, checks, directory);
        return 1;
    }
    const char *made[] = { "thousand", "lines", "big", "pieces", "failing", "seek-theirs", "seek-ours", "turn", "push",
                           "log", "newlog", "modes", "exit-1", "exit-2", "exit-3" };
    for (size_t n = 0; n < sizeof made / sizeof made[0]; n++)
        unlink(made[n]);
    if (chdir("/") != 0 || rmdir(directory) != 0)
        printf("(could not remove %s)\n", directory);
    printf("stdio: %d checks, all passed\n", checks);
    return 0;
}
