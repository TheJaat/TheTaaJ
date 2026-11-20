/* stdlib_test.c - ctype.c and stdlib.c against this machine's.
 *
 * Runs on the host, not in TheTaaJ. The two files are compiled for the
 * host as they are, and their names changed in the object files
 * (objcopy, with the list in host_names: taaj_strtol for strtol), so
 * that they sit in one program with the host's own. Then both are
 * asked the same things:
 *
 *   ctype    every question, about every value an argument may have:
 *            EOF, and 0 to 255
 *   strtol   a list of texts chosen to be awkward, in every base -
 *            the value, where the number ended, and errno
 *   qsort    thousands of arrays of random size and contents
 *
 * Built and run by `make libc-test`. */

#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int taaj_errno;

int taaj_isalpha(int c);
int taaj_isdigit(int c);
int taaj_isalnum(int c);
int taaj_isupper(int c);
int taaj_islower(int c);
int taaj_isxdigit(int c);
int taaj_isspace(int c);
int taaj_isblank(int c);
int taaj_isprint(int c);
int taaj_isgraph(int c);
int taaj_ispunct(int c);
int taaj_iscntrl(int c);
int taaj_toupper(int c);
int taaj_tolower(int c);
long taaj_strtol(const char *text, char **end, int base);
unsigned long taaj_strtoul(const char *text, char **end, int base);
int taaj_atoi(const char *text);
long taaj_atol(const char *text);
int taaj_abs(int number);
long taaj_labs(long number);
void taaj_qsort(void *base, size_t count, size_t size, int (*compare)(const void *, const void *));
void *taaj_bsearch(const void *key, const void *base, size_t count, size_t size,
                   int (*compare)(const void *, const void *));

static int checks;
static int failures;

#define CHECK(condition, ...)              \
    do {                                   \
        checks++;                          \
        if (!(condition)) {                \
            failures++;                    \
            printf("line %d: ", __LINE__); \
            printf(__VA_ARGS__);           \
            printf("\n");                  \
        }                                  \
    } while (0)

/* The host's ctype answers "yes" with any number that is not 0. */
#define SAME_KIND(name)                                                              \
    for (int c = -1; c <= 255; c++)                                                  \
        CHECK(!name(c) == !taaj_##name(c), #name "(%d): host %d, ours %d", c, name(c), taaj_##name(c))

#define SAME_CASE(name) \
    for (int c = -1; c <= 255; c++) CHECK(name(c) == taaj_##name(c), #name "(%d): host %d, ours %d", c, name(c), taaj_##name(c))

static uint32_t state = 2463534242u;

static uint32_t random_number(void)
{
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    return state;
}

static int compare_ints(const void *one, const void *other)
{
    int a = *(const int *)one, b = *(const int *)other;
    return (a > b) - (a < b);
}

static int compare_strings(const void *one, const void *other)
{
    return strcmp(*(const char *const *)one, *(const char *const *)other);
}

/* An item bigger than a register, of a size that is not a multiple of
 * anything: a key, and 19 bytes that must travel with it. */
struct record {
    unsigned char key;
    char cargo[19];
};

static int compare_records(const void *one, const void *other)
{
    return (int)((const struct record *)one)->key - (int)((const struct record *)other)->key;
}

int main(void)
{
    /* ctype: all of it. */
    SAME_KIND(isalpha);
    SAME_KIND(isdigit);
    SAME_KIND(isalnum);
    SAME_KIND(isupper);
    SAME_KIND(islower);
    SAME_KIND(isxdigit);
    SAME_KIND(isspace);
    SAME_KIND(isblank);
    SAME_KIND(isprint);
    SAME_KIND(isgraph);
    SAME_KIND(ispunct);
    SAME_KIND(iscntrl);
    SAME_CASE(toupper);
    SAME_CASE(tolower);

    /* strtol and strtoul. */
    static const char *const texts[] = {
        "0", "7", "42", "-42", "+42", "   42", "\t\n 42", "42abc", "42 ", "abc", "", " ", "-", "+", "--5", "+-5", "- 5",
        "0x", "0x1f", "0X1F", "0xg", "x1f", "-0x1f", "0x-1", "010", "08", "0", "00", "-0", "1e5", "z", "Zz", "10 20",
        "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809",
        "18446744073709551615", "18446744073709551616", "-18446744073709551615", "-18446744073709551616",
        "99999999999999999999999999", "-99999999999999999999999999", "0x7fffffffffffffff", "0x8000000000000000",
        "0xffffffffffffffff", "0x10000000000000000", "1111111111111111111111111111111111111111111111111111111111111111",
        "11111111111111111111111111111111111111111111111111111111111111111", "zzzzzzzzzzzzz", "1y2p0ij32e8e7",
        "1y2p0ij32e8e8", "0b101", "١٢٣", "12\00034",
    };
    static const int bases[] = { 0, 2, 3, 8, 10, 16, 17, 35, 36 };
    for (size_t t = 0; t < sizeof texts / sizeof texts[0]; t++) {
        for (size_t b = 0; b < sizeof bases / sizeof bases[0]; b++) {
            const char *text = texts[t];
            int base = bases[b];
            char *their_end, *our_end;
            errno = 0;
            long theirs = strtol(text, &their_end, base);
            int their_errno = errno;
            taaj_errno = 0;
            long ours = taaj_strtol(text, &our_end, base);
            CHECK(theirs == ours && their_end == our_end && their_errno == taaj_errno,
                  "strtol(\"%s\", %d): host %ld, end +%td, errno %d; ours %ld, end +%td, errno %d", text, base, theirs,
                  their_end - text, their_errno, ours, our_end - text, taaj_errno);
            errno = 0;
            unsigned long their_big = strtoul(text, &their_end, base);
            their_errno = errno;
            taaj_errno = 0;
            unsigned long our_big = taaj_strtoul(text, &our_end, base);
            CHECK(their_big == our_big && their_end == our_end && their_errno == taaj_errno,
                  "strtoul(\"%s\", %d): host %lu, end +%td, errno %d; ours %lu, end +%td, errno %d", text, base,
                  their_big, their_end - text, their_errno, our_big, our_end - text, taaj_errno);
        }
        CHECK(taaj_strtol(texts[t], NULL, 10) == strtol(texts[t], NULL, 10), "strtol(\"%s\") with nowhere to put the end", texts[t]);
    }
    static const int no_bases[] = { 1, -1, 37, 100 };
    for (size_t b = 0; b < sizeof no_bases / sizeof no_bases[0]; b++) {
        taaj_errno = 0;
        CHECK(taaj_strtol("12", NULL, no_bases[b]) == 0 && taaj_errno == EINVAL, "strtol in base %d: 0 and EINVAL", no_bases[b]);
        taaj_errno = 0;
        CHECK(taaj_strtoul("12", NULL, no_bases[b]) == 0 && taaj_errno == EINVAL, "strtoul in base %d: 0 and EINVAL", no_bases[b]);
    }
    taaj_errno = 0;
    CHECK(taaj_strtol("12", NULL, 10) == 12 && taaj_errno == 0, "a number that fits leaves errno alone");
    CHECK(taaj_atoi("  -17 apples") == -17 && taaj_atoi("none") == 0 && taaj_atol("123456789012") == 123456789012,
          "atoi and atol");
    CHECK(taaj_abs(-5) == 5 && taaj_abs(5) == 5 && taaj_abs(0) == 0 && taaj_abs(INT_MIN + 1) == INT_MAX, "abs");
    CHECK(taaj_labs(-5000000000) == 5000000000 && taaj_labs(LONG_MAX) == LONG_MAX, "labs");

    /* qsort: whole numbers, where equal items cannot be told apart, so
     * the result must be exactly the host's. Small ranges, to have
     * many that are equal. */
    for (int round = 0; round < 3000; round++) {
        static int theirs[300], ours[300];
        size_t count = random_number() % 300;
        int range = 1 + (int)(random_number() % (round % 3 == 0 ? 5 : 100000));
        for (size_t n = 0; n < count; n++)
            theirs[n] = ours[n] = (int)(random_number() % (unsigned)range) - range / 2;
        if (round % 7 == 0)
            qsort(ours, count, sizeof ours[0], compare_ints); /* already in order: quicksort's bad day */
        qsort(theirs, count, sizeof theirs[0], compare_ints);
        taaj_qsort(ours, count, sizeof ours[0], compare_ints);
        CHECK(memcmp(theirs, ours, count * sizeof ours[0]) == 0, "qsort of %zu ints, round %d", count, round);
    }

    /* Items that are not ints. */
    {
        const char *words[] = { "pear", "apple", "fig", "apple", "", "zebra", "Apple", "figs", "banana" };
        const char *sorted[sizeof words / sizeof words[0]];
        memcpy(sorted, words, sizeof words);
        qsort(words, 9, sizeof words[0], compare_strings);
        taaj_qsort(sorted, 9, sizeof sorted[0], compare_strings);
        int same = 1;
        for (int n = 0; n < 9; n++)
            same &= strcmp(words[n], sorted[n]) == 0;
        CHECK(same, "qsort of strings");

        /* Records: in order by key, and each key still with the cargo
         * it started with. */
        for (int round = 0; round < 200; round++) {
            static struct record records[150];
            size_t count = 1 + random_number() % 150;
            for (size_t n = 0; n < count; n++) {
                records[n].key = (unsigned char)(random_number() % 40);
                memset(records[n].cargo, records[n].key ^ 0x5a, sizeof records[n].cargo);
            }
            taaj_qsort(records, count, sizeof records[0], compare_records);
            int good = 1;
            for (size_t n = 0; n < count; n++) {
                good &= n == 0 || records[n - 1].key <= records[n].key;
                for (size_t at = 0; at < sizeof records[n].cargo; at++)
                    good &= (unsigned char)records[n].cargo[at] == (records[n].key ^ 0x5a);
            }
            CHECK(good, "qsort of %zu records of 20 bytes, round %d", count, round);
        }

        unsigned char bytes[] = { 9, 3, 200, 0, 3, 255, 1 };
        taaj_qsort(bytes, sizeof bytes, 1, compare_records);
        CHECK(memcmp(bytes, "\x00\x01\x03\x03\x09\xc8\xff", 7) == 0, "qsort of single bytes");
        int untouched[] = { 3, 1, 2 };
        taaj_qsort(untouched, 0, sizeof untouched[0], compare_ints);
        taaj_qsort(untouched, 1, sizeof untouched[0], compare_ints);
        taaj_qsort(untouched, 3, 0, compare_ints);
        CHECK(untouched[0] == 3 && untouched[1] == 1 && untouched[2] == 2, "qsort of nothing, of one item, and of items of no size");
    }

    /* bsearch: every item is found where it is, and what is not there
     * is not found. Odd numbers only, so the even ones are missing -
     * below, between and above. */
    for (int count = 0; count <= 40; count++) {
        int items[40];
        for (int n = 0; n < count; n++)
            items[n] = 2 * n + 1;
        int good = 1;
        for (int key = 0; key <= 2 * count + 1; key++) {
            int *theirs = bsearch(&key, items, (size_t)count, sizeof items[0], compare_ints);
            int *ours = taaj_bsearch(&key, items, (size_t)count, sizeof items[0], compare_ints);
            good &= theirs == ours;
        }
        CHECK(good, "bsearch among %d items", count);
    }

    if (failures != 0) {
        printf("stdlib: %d of %d checks FAILED\n", failures, checks);
        return 1;
    }
    printf("stdlib: %d checks, all the same as the host's\n", checks);
    return 0;
}
