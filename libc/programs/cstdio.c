/* cstdio - streams, from C: fopen, fgets, fread, fwrite, and buffers.
 *
 *   cstdio lines [FILE]    the file, or standard input, with its lines
 *                          numbered
 *   cstdio copy FROM TO    copies a file
 *   cstdio order           shows that printf waits and write does not
 *   cstdio end             prints with no new line, and returns
 *   cstdio lost            prints, and ends with _exit
 *   cstdio check           checks streams against files it makes and
 *                          removes (cstdio.tmp, cstdio.new)
 *
 * As with cfile, the files are there only for a program started with
 * them:
 *
 *   with readonly cstdio lines welcome
 *   with files cstdio check */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Each line with its number in front. A line longer than the room
 * arrives in pieces, and only the first piece is a line's beginning. */
static int number_lines(FILE *from)
{
    char piece[128];
    int line = 0;
    int at_start = 1;
    while (fgets(piece, sizeof piece, from) != NULL) {
        if (at_start)
            printf("%4d  ", ++line);
        fputs(piece, stdout);
        at_start = piece[strlen(piece) - 1] == '\n';
    }
    return ferror(from) ? -1 : 0;
}

static int lines(const char *name)
{
    if (name == NULL)
        return number_lines(stdin) < 0;
    FILE *file = fopen(name, "r");
    if (file == NULL || number_lines(file) < 0) {
        fprintf(stderr, "cstdio: %s: %s\n", name, strerror(errno));
        return 1;
    }
    fclose(file);
    return 0;
}

static int copy(const char *from_name, const char *to_name)
{
    FILE *from = fopen(from_name, "r");
    if (from == NULL) {
        fprintf(stderr, "cstdio: %s: %s\n", from_name, strerror(errno));
        return 1;
    }
    FILE *to = fopen(to_name, "w");
    if (to == NULL) {
        fprintf(stderr, "cstdio: %s: %s\n", to_name, strerror(errno));
        return 1;
    }
    /* A size that fits neither the buffer nor a message, on purpose. */
    char piece[1000];
    size_t count;
    long total = 0;
    while ((count = fread(piece, 1, sizeof piece, from)) > 0) {
        if (fwrite(piece, 1, count, to) != count)
            break;
        total += (long)count;
    }
    /* fclose is where the last of the file is written: its answer is
     * part of whether the copy worked. */
    if (ferror(from) || ferror(to) || fclose(to) == EOF) {
        fprintf(stderr, "cstdio: copying %s to %s: %s\n", from_name, to_name, strerror(errno));
        return 1;
    }
    fclose(from);
    printf("%s: %ld bytes\n", to_name, total);
    return 0;
}

static int passed;
static int failed;

static void check(int held, const char *what)
{
    if (held) {
        passed++;
        return;
    }
    failed++;
    fprintf(stderr, "cstdio: failed: %s (errno %d: %s)\n", what, errno, strerror(errno));
}

#define SCRATCH "cstdio.tmp"
#define MOVED "cstdio.new"

/* How big the file is now, asked of the file service - not of any
 * stream. */
static long size_on_disk(const char *path)
{
    int file = open(path, O_RDONLY);
    if (file < 0)
        return -1;
    long size = lseek(file, 0, SEEK_END);
    close(file);
    return size;
}

static int check_streams(void)
{
    char line[64], wanted[64];

    /* What cannot be opened. */
    remove(SCRATCH);
    remove(MOVED);
    errno = 0;
    check(fopen(SCRATCH, "r") == NULL && errno == ENOENT, "fopen to read what is not there");
    check(fopen(SCRATCH, "both") == NULL && errno == EINVAL, "fopen with a mode that is not one");
    check(fopen("/", "r") == NULL && errno == EISDIR, "fopen of a directory");
    check(remove(SCRATCH) == -1 && errno == ENOENT, "remove of what is not there");

    /* Writing: 200 lines, which wait in the stream. */
    FILE *out = fopen(SCRATCH, "w");
    check(out != NULL, "fopen to write");
    check(fileno(out) == 3, "its descriptor is the lowest not in use");
    int printed = 0;
    for (int n = 0; n < 200; n++)
        printed += fprintf(out, "line %d\n", n);
    check(printed == 1690, "fprintf returns the lengths");
    check(size_on_disk(SCRATCH) == 0, "nothing has reached the file yet");
    check(ftell(out) == 1690, "though ftell counts it");
    check(fflush(out) == 0 && size_on_disk(SCRATCH) == 1690, "fflush sends it");
    fputs("tail", out);
    check(fclose(out) == 0 && size_on_disk(SCRATCH) == 1694, "fclose sends the rest");
    check(fopen(SCRATCH, "wx") == NULL && errno == EEXIST, "\"wx\" on a file that is there");
    check(size_on_disk(SCRATCH) == 1694, "which is not emptied by being refused");

    /* Reading it back in lines. */
    FILE *in = fopen(SCRATCH, "r");
    check(in != NULL, "fopen to read");
    check(fgetc(in) == 'l' && ftell(in) == 1, "one fgetc: the stream is at 1");
    check(lseek(fileno(in), 0, SEEK_CUR) > 1, "and the descriptor is further on: bytes were read ahead");
    check(ungetc('l', in) == 'l' && ftell(in) == 0, "ungetc puts it back");
    int same = 1;
    for (int n = 0; n < 200; n++) {
        snprintf(wanted, sizeof wanted, "line %d\n", n);
        same &= fgets(line, sizeof line, in) != NULL && strcmp(line, wanted) == 0;
    }
    check(same, "fgets gives the 200 lines");
    check(fgets(line, sizeof line, in) != NULL && strcmp(line, "tail") == 0, "and a last one with no new line");
    check(fgets(line, sizeof line, in) == NULL && feof(in) && !ferror(in), "then NULL: the end, not an error");
    errno = 0;
    check(fputc('x', in) == EOF && ferror(in) && errno == EBADF, "writing to a stream opened to read");
    clearerr(in);

    /* Moving. Lines 0 to 9 are 7 bytes each and 10 to 99 are 8: line
     * 100 starts at 790. */
    check(fseek(in, 790, SEEK_SET) == 0 && fgets(line, sizeof line, in) != NULL && strcmp(line, "line 100\n") == 0,
          "fseek to a line, and fgets reads it");
    check(ftell(in) == 799, "ftell after it");
    check(fseek(in, -9, SEEK_CUR) == 0 && ftell(in) == 790 && fgetc(in) == 'l', "fseek back from here");
    check(fseek(in, -4, SEEK_END) == 0 && fread(line, 1, sizeof line, in) == 4 && memcmp(line, "tail", 4) == 0,
          "fseek from the end; fread stops at the end");
    rewind(in);
    check(ftell(in) == 0 && !feof(in), "rewind");
    check(fclose(in) == 0, "fclose");

    /* Both ways on one stream. */
    FILE *both = fopen(SCRATCH, "r+");
    check(both != NULL && fseek(both, 790, SEEK_SET) == 0 && fputs("LINE", both) != EOF, "\"r+\": write in the middle");
    check(fseek(both, 790, SEEK_SET) == 0 && fgets(line, sizeof line, both) != NULL && strcmp(line, "LINE 100\n") == 0,
          "and read it back on the same stream");
    check(fgetc(both) == 'l' && fputc('L', both) == 'L' && fgetc(both) == 'n', "read, write, read, with no fseek between");
    check(fclose(both) == 0 && size_on_disk(SCRATCH) == 1694, "the file is no longer or shorter");
    in = fopen(SCRATCH, "r");
    check(fseek(in, 799, SEEK_SET) == 0 && fgets(line, sizeof line, in) != NULL && strcmp(line, "lLne 101\n") == 0,
          "and the one byte written went where the reading was");
    fclose(in);

    /* Adding at the end. */
    FILE *more = fopen(SCRATCH, "a+");
    check(more != NULL && fgets(line, sizeof line, more) != NULL && strcmp(line, "line 0\n") == 0, "\"a+\" reads from the start");
    check(fputs("\nadded\n", more) != EOF && fclose(more) == 0 && size_on_disk(SCRATCH) == 1701, "and writes at the end");

    /* More than a buffer, and more than a message, in items. */
    enum { BIG = 10000 };
    unsigned char *sent = malloc(BIG);
    unsigned char *back = malloc(BIG);
    for (int n = 0; n < BIG; n++)
        sent[n] = (unsigned char)(n * 7 + n / 256);
    out = fopen(SCRATCH, "w");
    check(fwrite(sent, 4, BIG / 4, out) == BIG / 4, "fwrite of 2500 items of 4");
    check(fclose(out) == 0 && size_on_disk(SCRATCH) == BIG, "is 10000 bytes in the file");
    in = fopen(SCRATCH, "r");
    check(fread(back, 1, 3, in) == 3 && fread(back + 3, 7, 1000, in) == 1000 && fread(back + 7003, 1, BIG, in) == BIG - 7003,
          "fread in three unequal parts, the last cut short by the end");
    check(feof(in) && memcmp(sent, back, BIG) == 0, "gives back the same bytes");
    fclose(in);
    free(sent);
    free(back);

    /* Only so many at once, and fclose gives each back. */
    FILE *held[40];
    int count = 0;
    while (count < 40 && (held[count] = fopen(SCRATCH, "r")) != NULL)
        count++;
    check(count == 29 && errno == EMFILE, "29 streams, with the three every program has: 32");
    while (count > 0)
        fclose(held[--count]);
    in = fopen(SCRATCH, "r");
    check(in != NULL && fileno(in) == 3, "and all of them come back");
    fclose(in);

    /* A new name. */
    check(rename(SCRATCH, MOVED) == 0, "rename");
    check(fopen(SCRATCH, "r") == NULL && errno == ENOENT, "the old name is gone");
    check(size_on_disk(MOVED) == BIG, "and the file is under the new one");
    out = fopen(SCRATCH, "w");
    fclose(out);
    errno = 0;
    check(rename(MOVED, SCRATCH) == -1 && errno == EEXIST, "rename onto a file that is there replaces nothing");
    check(rename("cstdio.none", "cstdio.other") == -1 && errno == ENOENT, "rename of what is not there");
    check(remove(SCRATCH) == 0 && remove(MOVED) == 0, "remove");
    check(size_on_disk(SCRATCH) == -1 && size_on_disk(MOVED) == -1, "both are gone");

    /* What the three are. */
    errno = 0;
    check(isatty(STDERR_FILENO) == 1, "standard error is the screen");
    check(isatty(STDIN_FILENO) == 0 && errno == ENOTTY, "standard input is not");
    check(isatty(99) == 0 && errno == EBADF, "and 99 is nothing");
    check(fseek(stdout, 0, SEEK_SET) == -1 && errno == ESPIPE, "standard output has no place to move to");
    /* This is the first thing the program prints: the moment the
     * library asks whether standard output is the screen. */
    errno = 0;
    printf("streams: ");
    check(errno == 0, "printf, asking whether it is the screen, leaves errno alone");
    printf("%d checks", passed);
    if (failed != 0)
        printf(", %d FAILED\n", failed);
    else
        printf(", all passed\n");
    return failed != 0;
}

int main(int argc, char **argv)
{
    if (argc >= 2 && argc <= 3 && strcmp(argv[1], "lines") == 0)
        return lines(argv[2]);
    if (argc == 4 && strcmp(argv[1], "copy") == 0)
        return copy(argv[2], argv[3]);
    if (argc == 2 && strcmp(argv[1], "order") == 0) {
        /* printf's text waits for the end of its line; write's does
         * not wait for anything. */
        static const char now[] = "write was called second - ";
        printf("printf was called first");
        write(STDOUT_FILENO, now, sizeof now - 1);
        printf("\n");
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "end") == 0) {
        printf("no new line, and still here");
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "lost") == 0) {
        printf("kept\n");
        fflush(stdout);
        printf("this is lost");
        _exit(0);
    }
    if (argc == 2 && strcmp(argv[1], "check") == 0) {
        FILE *scratch = fopen(SCRATCH, "w");
        if (scratch == NULL) {
            perror("cstdio: check needs to make a file (with files cstdio check)");
            return 2;
        }
        fclose(scratch);
        return check_streams();
    }
    fprintf(stderr, "usage: cstdio lines [FILE] | copy FROM TO | order | end | lost | check\n");
    return 2;
}
