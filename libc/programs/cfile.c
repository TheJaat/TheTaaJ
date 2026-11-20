/* cfile - files, from C: open, read, write, lseek, close, unlink.
 *
 *   cfile cat [FILE...]       the files, or standard input, to standard
 *                             output
 *   cfile write FILE WORD...  makes FILE hold the words and a new line
 *   cfile check               checks descriptors against a file it
 *                             makes and removes (cfile.tmp)
 *
 * A program is given the files only by whoever starts it:
 *
 *   with readonly cfile cat welcome
 *   with files cfile check
 *
 * Without that, open fails - and says why. */

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* One descriptor, to its end, to standard output. */
static int copy_out(int from)
{
    char piece[512];
    ssize_t count;
    while ((count = read(from, piece, sizeof piece)) > 0) {
        if (write(STDOUT_FILENO, piece, (size_t)count) < 0)
            return -1;
    }
    return (int)count; /* 0 at the end, -1 if a read failed */
}

static int cat(int count, char **names)
{
    if (count == 0)
        return copy_out(STDIN_FILENO) < 0;
    int failed = 0;
    for (int n = 0; n < count; n++) {
        int file = open(names[n], O_RDONLY);
        if (file < 0 || copy_out(file) < 0) {
            fprintf(stderr, "cfile: %s: %s\n", names[n], strerror(errno));
            failed = 1;
        }
        if (file >= 0)
            close(file);
    }
    return failed;
}

static int write_words(const char *name, int count, char **words)
{
    int file = open(name, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (file < 0) {
        perror(name);
        return 1;
    }
    size_t total = 0;
    for (int n = 0; n < count; n++) {
        const char *after = n + 1 < count ? " " : "\n";
        if (write(file, words[n], strlen(words[n])) < 0 || write(file, after, 1) < 0) {
            perror(name);
            return 1;
        }
        total += strlen(words[n]) + 1;
    }
    close(file);
    printf("%s: %zu bytes\n", name, total);
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
    fprintf(stderr, "cfile: failed: %s (errno %d: %s)\n", what, errno, strerror(errno));
}

/* The call failed, and for this reason. */
#define FAILS(call, reason, what) (errno = 0, check((call) == -1 && errno == (reason), what))

#define SCRATCH "cfile.tmp"

static int check_files(void)
{
    char room[64];

    /* What is not there, and what is not a descriptor. */
    unlink(SCRATCH);
    FAILS(open(SCRATCH, O_RDONLY), ENOENT, "opening what is not there");
    FAILS(unlink(SCRATCH), ENOENT, "removing what is not there");
    FAILS(read(99, room, 1), EBADF, "reading a descriptor that is not one");
    FAILS(write(-1, room, 1), EBADF, "writing a descriptor that is not one");
    FAILS(close(99), EBADF, "closing a descriptor that is not one");
    FAILS(open("/", O_RDONLY), EISDIR, "opening a directory");
    FAILS(open(SCRATCH, 3), EINVAL, "opening for neither reading nor writing");

    /* Making a file, and writing it. */
    int out = open(SCRATCH, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check(out == 3, "open gives the lowest number not in use");
    check(write(out, "hello, world\n", 13) == 13, "write");
    FAILS(read(out, room, 1), EBADF, "reading what was opened for writing");
    check(close(out) == 0, "close");
    FAILS(close(out), EBADF, "closing twice");
    FAILS(open(SCRATCH, O_WRONLY | O_CREAT | O_EXCL, 0644), EEXIST, "O_EXCL on a file that is there");

    /* Reading it: a piece, the rest, and then the end. */
    int in = open(SCRATCH, O_RDONLY);
    check(in == 3, "a closed number is given out again");
    check(read(in, room, 5) == 5 && memcmp(room, "hello", 5) == 0, "read takes what was asked for");
    check(read(in, room, sizeof room) == 8 && memcmp(room, ", world\n", 8) == 0, "read goes on from there, to the end");
    check(read(in, room, sizeof room) == 0, "read at the end gives 0");
    check(read(in, room, sizeof room) == 0, "and again");
    FAILS(write(in, "x", 1), EBADF, "writing what was opened for reading");

    /* Moving about in it. */
    check(lseek(in, 0, SEEK_END) == 13, "lseek to the end says the size");
    check(lseek(in, 7, SEEK_SET) == 7, "lseek from the start");
    check(read(in, room, 5) == 5 && memcmp(room, "world", 5) == 0, "read from where lseek left it");
    check(lseek(in, -5, SEEK_CUR) == 7, "lseek back from here");
    check(lseek(in, -6, SEEK_END) == 7, "lseek back from the end");
    FAILS(lseek(in, -1, SEEK_SET), EINVAL, "lseek to before the start");
    FAILS(lseek(in, 0, 7), EINVAL, "lseek from nowhere");
    FAILS(lseek(STDOUT_FILENO, 0, SEEK_SET), ESPIPE, "lseek on a stream");

    /* Two descriptors for one file each keep their own place. */
    int other = open(SCRATCH, O_RDONLY);
    check(other == 4, "a second descriptor is the next number");
    check(read(other, room, 1) == 1 && room[0] == 'h', "it starts at the start");
    check(read(in, room, 1) == 1 && room[0] == 'w', "and the first is where it was");
    close(other);
    close(in);

    /* Writing in the middle of what is there. */
    int both = open(SCRATCH, O_RDWR);
    check(lseek(both, 7, SEEK_SET) == 7 && write(both, "there", 5) == 5, "write in the middle");
    check(lseek(both, 0, SEEK_SET) == 0 && read(both, room, sizeof room) == 13 && memcmp(room, "hello, there\n", 13) == 0,
          "and only the middle changed");
    close(both);

    /* O_APPEND: at the end, wherever the descriptor was moved to. */
    int end = open(SCRATCH, O_WRONLY | O_APPEND);
    lseek(end, 0, SEEK_SET);
    check(write(end, "more\n", 5) == 5, "write with O_APPEND");
    check(lseek(end, 0, SEEK_CUR) == 18, "it went at the end");
    close(end);
    in = open(SCRATCH, O_RDONLY);
    check(read(in, room, sizeof room) == 18 && memcmp(room, "hello, there\nmore\n", 18) == 0, "and nothing was written over");
    close(in);

    /* Past the end: zeros in between. */
    both = open(SCRATCH, O_RDWR);
    check(lseek(both, 20, SEEK_SET) == 20 && write(both, "!", 1) == 1, "write past the end");
    check(lseek(both, 18, SEEK_SET) == 18 && read(both, room, sizeof room) == 3 && memcmp(room, "\0\0!", 3) == 0,
          "leaves zeros in the gap");
    close(both);

    /* O_TRUNC empties. */
    out = open(SCRATCH, O_WRONLY | O_TRUNC);
    check(lseek(out, 0, SEEK_END) == 0, "O_TRUNC empties the file");

    /* More than one message's worth, in one write; read back in
     * pieces that fit nothing in particular. */
    enum { BIG = 5000 };
    unsigned char *sent = malloc(BIG);
    unsigned char *back = malloc(BIG);
    for (int n = 0; n < BIG; n++)
        sent[n] = (unsigned char)(n * 7 + n / 256);
    check(write(out, sent, BIG) == BIG, "a write of 5000 bytes");
    close(out);
    in = open(SCRATCH, O_RDONLY);
    size_t got = 0;
    ssize_t count;
    while (got < BIG && (count = read(in, back + got, 700)) > 0)
        got += (size_t)count;
    check(got == BIG && memcmp(sent, back, BIG) == 0, "comes back the same");
    check(read(in, back, 700) == 0, "and then the end");
    close(in);
    free(sent);
    free(back);

    /* Only so many at once - and a failure costs none of them. */
    int opened[40];
    int held = 0;
    while (held < 40 && (opened[held] = open(SCRATCH, O_RDONLY)) >= 0)
        held++;
    check(held == 29 && errno == EMFILE, "32 descriptors at most, the three standard ones among them");
    while (held > 0)
        close(opened[--held]);
    in = open(SCRATCH, O_RDONLY);
    check(in == 3, "and all of them come back");
    close(in);

    /* Removing it. */
    check(unlink(SCRATCH) == 0, "unlink");
    FAILS(open(SCRATCH, O_RDONLY), ENOENT, "it is gone");

    /* errno is left alone by a call that works. */
    errno = 0;
    out = open(SCRATCH, O_WRONLY | O_CREAT, 0644);
    close(out);
    unlink(SCRATCH);
    check(errno == 0, "a call that works does not touch errno");
    FAILS(malloc((size_t)1 << 40) == NULL ? -1 : 0, ENOMEM, "malloc says ENOMEM");
    check(strcmp(strerror(ENOENT), "no such file or directory") == 0, "strerror");

    printf("files: %d checks", passed);
    if (failed != 0)
        printf(", %d FAILED\n", failed);
    else
        printf(", all passed\n");
    return failed != 0;
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "cat") == 0)
        return cat(argc - 2, argv + 2);
    if (argc > 3 && strcmp(argv[1], "write") == 0)
        return write_words(argv[2], argc - 3, argv + 3);
    if (argc == 2 && strcmp(argv[1], "check") == 0) {
        int scratch = open(SCRATCH, O_WRONLY | O_CREAT, 0644);
        if (scratch < 0) {
            perror("cfile: check needs to make a file (with files cfile check)");
            return 2;
        }
        close(scratch);
        return check_files();
    }
    fprintf(stderr, "usage: cfile cat [FILE...] | write FILE WORD... | check\n");
    return 2;
}
