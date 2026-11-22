/* scan_test.c - the library's sscanf, on this machine.
 *
 * Runs on the host, not in TheTaaJ. scan.c is compiled for the host
 * with sscanf and vsscanf renamed taaj_sscanf and taaj_vsscanf, so that
 * it lives in one program with glibc's; what it calls (strtol, strtod,
 * isspace) is glibc's, tested elsewhere. Each case is given to both,
 * and the two must store the same things and return the same count.
 *
 * Built and run by `make libc-test`. */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int taaj_sscanf(const char *text, const char *format, ...);

static int checks, failures;

static void fail(const char *text, const char *format, const char *what)
{
    failures++;
    if (failures < 20)
        printf("FAIL: sscanf(\"%s\", \"%s\"): %s\n", text, format, what);
}

/* One conversion of type T: both scanf the text, and must agree on the
 * count and, if something was stored, on its value (by its bytes). */
#define ONE(T, text, format)                                                     \
    do {                                                                         \
        T a, b;                                                                  \
        memset(&a, 0x5a, sizeof a);                                              \
        memset(&b, 0x5a, sizeof b);                                              \
        int ra = taaj_sscanf(text, format, &a), rb = sscanf(text, format, &b);   \
        checks++;                                                                \
        if (ra != rb)                                                            \
            fail(text, format, "a different count");                             \
        else if (ra == 1 && memcmp(&a, &b, sizeof a) != 0)                       \
            fail(text, format, "a different value");                             \
    } while (0)

/* A string conversion: the strings must be the same. */
static void string_case(const char *text, const char *format)
{
    /* %3c with fewer than 3 characters left: C says the conversion
     * fails (input runs out inside it), glibc stores what there is and
     * counts it. Ours does what C says. */
    if (strcmp(format, "%3c") == 0 && strlen(text) < 3)
        return;
    char a[64], b[64];
    memset(a, 'A', sizeof a);
    memset(b, 'A', sizeof b);
    int ra = taaj_sscanf(text, format, a), rb = sscanf(text, format, b);
    checks++;
    if (ra != rb)
        fail(text, format, "a different count");
    else if (ra == 1 && memcmp(a, b, sizeof a) != 0)
        fail(text, format, "a different string");
}

/* %n after other conversions: how far each got. */
static void position_case(const char *text, const char *format)
{
    int x1 = 0, x2 = 0, n1 = -1, n2 = -1;
    int ra = taaj_sscanf(text, format, &x1, &n1), rb = sscanf(text, format, &x2, &n2);
    checks++;
    if (ra != rb || x1 != x2 || n1 != n2)
        fail(text, format, "a different count, value or position");
}

int main(void)
{
    static const char *integers[] = {
        "0", "42", "-42", "+7", "  123abc", "0x1f", "-0x10", "0777", "08", "9223372036854775807",
        "-9223372036854775808", "18446744073709551615", "", "   ", "x", "-", "0x", "12 34",
    };
    static const char *integer_formats[] = {"%d", "%i", "%u", "%x", "%o", "%3d", "%1d", "%2x", "%*d"};
    for (size_t i = 0; i < sizeof integers / sizeof integers[0]; i++)
        for (size_t f = 0; f < sizeof integer_formats / sizeof integer_formats[0]; f++)
            ONE(int, integers[i], integer_formats[f]);
    static const char *long_formats[] = {"%ld", "%lli", "%lu", "%llx", "%zd", "%jd"};
    for (size_t i = 0; i < sizeof integers / sizeof integers[0]; i++)
        for (size_t f = 0; f < sizeof long_formats / sizeof long_formats[0]; f++)
            ONE(long long, integers[i], long_formats[f]);
    for (size_t i = 0; i < sizeof integers / sizeof integers[0]; i++) {
        ONE(short, integers[i], "%hd");
        ONE(signed char, integers[i], "%hhd");
    }

    static const char *floats[] = {
        "0", "3.5", "-2.25e3", "1e-310", "1e400", "inf", "-INFINITY", "nan", "0x1.8p1", ".5", "5.",
        "1.7976931348623157e308", "  6.02214076e23 mol", "e5", "-", "",
    };
    for (size_t i = 0; i < sizeof floats / sizeof floats[0]; i++) {
        ONE(double, floats[i], "%lf");
        ONE(float, floats[i], "%f");
        ONE(float, floats[i], "%g");
        ONE(double, floats[i], "%4lf");
        if (strstr(floats[i], "nan") == NULL) /* a long double NaN's padding bytes differ */
            ONE(long double, floats[i], "%Lf");
    }

    static const char *words[] = {"hello world", "  leading", "", "tab\tseparated", "abc123def", "]x", "a-z"};
    static const char *string_formats[] = {"%s", "%5s", "%c", "%3c", "%[a-z]", "%[^ ]", "%[]x]", "%[a-c]", "%[-az]", "%2[a-z]"};
    for (size_t i = 0; i < sizeof words / sizeof words[0]; i++)
        for (size_t f = 0; f < sizeof string_formats / sizeof string_formats[0]; f++)
            string_case(words[i], string_formats[f]);

    position_case("12 34", "%d%n");
    position_case("  12xyz", "%d%n");
    position_case("12", "%d %n");
    position_case("a12", "a%d%n");
    position_case("b12", "a%d%n");
    position_case("100%", "%d%%%n");

    /* Several at once, and the count stopping at the first mismatch. */
    int a1 = 0, a2 = 0, b1 = 0, b2 = 0;
    char w1[16] = "", w2[16] = "";
    checks++;
    if (taaj_sscanf("7, seven: 8", "%d, %15[a-z]: %d", &a1, w1, &b1) != sscanf("7, seven: 8", "%d, %15[a-z]: %d", &a2, w2, &b2)
        || a1 != a2 || b1 != b2 || strcmp(w1, w2) != 0)
        fail("7, seven: 8", "%d, %15[a-z]: %d", "differs");
    checks++;
    if (taaj_sscanf("1 x", "%d %d", &a1, &b1) != sscanf("1 x", "%d %d", &a2, &b2))
        fail("1 x", "%d %d", "a different count");

    /* Random numbers, printed then read back. */
    uint64_t state = 0x2545f4914f6cdd1dULL;
    for (int i = 0; i < 100000; i++) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        char text[64];
        snprintf(text, sizeof text, "%lld", (long long)state);
        ONE(long long, text, "%lld");
        double d;
        uint64_t bits = state & 0x7fefffffffffffffULL; /* finite */
        memcpy(&d, &bits, sizeof d);
        snprintf(text, sizeof text, "%.17g", d);
        ONE(double, text, "%lf");
    }

    printf("scan: %d checks, %s\n", checks, failures ? "some FAILED" : "all the same as the host's sscanf");
    return failures != 0;
}
