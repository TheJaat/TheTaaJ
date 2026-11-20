/* cmem - memory and strings, used from C.
 *
 *   cmem                 checks string.h and malloc, and says how many
 *                        checks passed
 *   cmem sort WORD...    the words in order, on one line
 *   cmem huge            asks for more memory than there is
 *
 * The checks are here, and not only in the host's tests, because here
 * they go through everything: the header a C compiler read, the linker
 * finding the name in libc.a, and the heap of a real process.
 *
 * There is still no printf, so it writes its numbers itself. */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void print(const char *text)
{
    write(STDOUT_FILENO, text, strlen(text));
}

static void complain(const char *text)
{
    write(STDERR_FILENO, text, strlen(text));
}

static void print_number(unsigned long value)
{
    char digits[21];
    size_t at = sizeof digits;
    do {
        digits[--at] = (char)('0' + value % 10);
        value /= 10;
    } while (value > 0);
    write(STDOUT_FILENO, digits + at, sizeof digits - at);
}

static unsigned long passed;
static unsigned long failed;

/* One check: counted, and named on standard error if it fails. */
static void check(int held, const char *what)
{
    if (held) {
        passed++;
        return;
    }
    failed++;
    complain("cmem: failed: ");
    complain(what);
    complain("\n");
}

/* Says how a group of checks went, and starts the count again. */
static int report(const char *group)
{
    int bad = failed != 0;
    print(group);
    print(": ");
    print_number(passed);
    print(" checks");
    if (bad) {
        print(", ");
        print_number(failed);
        print(" FAILED\n");
    } else {
        print(", all passed\n");
    }
    passed = 0;
    failed = 0;
    return bad;
}

static int check_strings(void)
{
    char room[32];

    check(strlen("") == 0, "strlen of nothing");
    check(strlen("hello") == 5, "strlen");
    check(strnlen("hello", 3) == 3, "strnlen stops at its limit");
    check(strnlen("hello", 30) == 5, "strnlen stops at the zero");

    check(strcmp("apple", "apple") == 0, "strcmp of the same");
    check(strcmp("apple", "fig") < 0, "strcmp, before");
    check(strcmp("fig", "apple") > 0, "strcmp, after");
    check(strcmp("app", "apple") < 0, "strcmp, the start of a string");
    check(strcmp("\xe9", "z") > 0, "strcmp compares as unsigned");
    check(strncmp("apple", "apricot", 2) == 0, "strncmp within its count");
    check(strncmp("apple", "apricot", 3) < 0, "strncmp at its count");

    check(strcpy(room, "one") == room, "strcpy returns where it copied to");
    check(strcmp(room, "one") == 0, "strcpy");
    strcat(room, " two");
    check(strcmp(room, "one two") == 0, "strcat");
    strncat(room, " three", 3);
    check(strcmp(room, "one two th") == 0, "strncat stops at its count");
    memset(room, '#', sizeof room);
    strncpy(room, "fig", 6);
    check(memcmp(room, "fig\0\0\0#", 7) == 0, "strncpy pads with zeros");

    const char *path = "/docs/deep/notes";
    check(strchr(path, '/') == path, "strchr finds the first");
    check(strrchr(path, '/') == path + 10, "strrchr finds the last");
    check(strchr(path, 'x') == NULL, "strchr of what is not there");
    check(strchr(path, '\0') == path + 16, "strchr finds the zero");
    check(strstr(path, "deep") == path + 6, "strstr");
    check(strstr(path, "shallow") == NULL, "strstr of what is not there");
    check(strstr(path, "") == path, "strstr of nothing");

    char bytes[8] = { 1, 2, 0, 4, 5, 6, 7, 8 };
    check(memchr(bytes, 4, sizeof bytes) == bytes + 3, "memchr goes past a zero");
    check(memchr(bytes, 9, sizeof bytes) == NULL, "memchr of what is not there");
    memmove(bytes + 1, bytes, 7);
    check(bytes[1] == 1 && bytes[2] == 2 && bytes[4] == 4 && bytes[7] == 7, "memmove over itself");
    memcpy(room, bytes, sizeof bytes);
    check(memcmp(room, bytes, sizeof bytes) == 0, "memcpy, and memcmp of the same");
    room[7] = 9;
    check(memcmp(room, bytes, sizeof bytes) > 0, "memcmp of a difference");

    return report("strings");
}

static int check_memory(void)
{
    /* Two blocks are two different places, and writing all of one
     * leaves the other alone. */
    unsigned char *a = malloc(100);
    unsigned char *b = malloc(100);
    check(a != NULL && b != NULL, "malloc");
    check(a != b, "two blocks are two places");
    check((uintptr_t)a % 16 == 0 && (uintptr_t)b % 16 == 0, "blocks are aligned to 16");
    memset(a, 0xaa, 100);
    memset(b, 0xbb, 100);
    check(a[0] == 0xaa && a[99] == 0xaa && b[0] == 0xbb && b[99] == 0xbb, "blocks do not overlap");
    free(a);
    free(b);
    free(NULL);

    /* A block of nothing is still a block. */
    void *nothing = malloc(0);
    check(nothing != NULL, "malloc(0)");
    free(nothing);

    /* calloc: zeros, though the memory has been used before - and no
     * block when count times size is too much to count. */
    unsigned char *dirty = malloc(4096);
    memset(dirty, 0xff, 4096);
    free(dirty);
    unsigned char *zeros = calloc(64, 64);
    int clean = zeros != NULL;
    for (size_t n = 0; clean && n < 4096; n++)
        clean = zeros[n] == 0;
    check(clean, "calloc gives zeros");
    free(zeros);
    check(calloc(SIZE_MAX / 2, 4) == NULL, "calloc refuses a count that overflows");

    /* realloc: the bytes come along, whether the block grows, shrinks
     * or moves. Grown one byte at a time to a megabyte's worth of
     * doublings. */
    size_t size = 1;
    unsigned char *grown = malloc(size);
    grown[0] = 7;
    int kept = 1;
    while (size < 1024 * 1024) {
        grown = realloc(grown, size * 2);
        if (grown == NULL)
            break;
        for (size_t n = 0; kept && n < size; n++)
            kept = grown[n] == (unsigned char)(n * 31 + 7);
        for (size_t n = size; n < size * 2; n++)
            grown[n] = (unsigned char)(n * 31 + 7);
        size *= 2;
    }
    check(grown != NULL && size == 1024 * 1024, "realloc grows to a megabyte");
    check(kept, "realloc keeps what was there");
    grown = realloc(grown, 10);
    check(grown != NULL && grown[9] == (unsigned char)(9 * 31 + 7), "realloc shrinks");
    check(realloc(grown, 0) == NULL, "realloc to nothing frees");
    unsigned char *fresh = realloc(NULL, 10);
    check(fresh != NULL, "realloc of NULL is malloc");
    free(fresh);

    /* free really gives memory back: 64 KiB taken and returned twenty
     * thousand times is 1.3 GiB, more than this machine has. If free
     * did nothing, malloc would run out. */
    int returned = 1;
    for (int round = 0; returned && round < 20000; round++) {
        unsigned char *block = malloc(64 * 1024);
        if (block == NULL) {
            returned = 0;
            break;
        }
        block[0] = 1;
        block[64 * 1024 - 1] = 1;
        free(block);
    }
    check(returned, "free gives memory back");

    char *copy = strdup("a copy");
    check(copy != NULL && strcmp(copy, "a copy") == 0, "strdup");
    copy[0] = 'A';
    check(strcmp(copy, "A copy") == 0, "strdup's copy can be written");
    free(copy);

    return report("memory");
}

/* The words in order: a list that grows as words arrive (realloc),
 * each one a copy of its own (strdup), put in order by strcmp. */
static int sort(int count, char **words)
{
    char **list = NULL;
    int held = 0;
    for (int n = 0; n < count; n++) {
        char **bigger = realloc(list, (size_t)(held + 1) * sizeof *list);
        char *copy = strdup(words[n]);
        if (bigger == NULL || copy == NULL) {
            complain("cmem: no memory\n");
            return 1;
        }
        list = bigger;
        /* Insertion sort: move the later words up one, and put this
         * one where it belongs. */
        int at = held;
        while (at > 0 && strcmp(list[at - 1], copy) > 0) {
            list[at] = list[at - 1];
            at--;
        }
        list[at] = copy;
        held++;
    }
    for (int n = 0; n < held; n++) {
        print(n == 0 ? "" : " ");
        print(list[n]);
        free(list[n]);
    }
    print("\n");
    free(list);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "sort") == 0)
        return sort(argc - 2, argv + 2);
    if (argc > 1 && strcmp(argv[1], "huge") == 0) {
        /* A terabyte. The answer must be NULL - not a crash, and not
         * an address that fails later when it is written to. */
        size_t terabyte = (size_t)1 << 40;
        void *block = malloc(terabyte);
        if (block == NULL) {
            complain("cmem: no memory for a terabyte\n");
            return 1;
        }
        print("a terabyte, apparently\n");
        return 0;
    }
    if (argc > 1) {
        complain("usage: cmem [sort WORD... | huge]\n");
        return 2;
    }
    int bad = check_strings();
    bad |= check_memory();
    return bad;
}
