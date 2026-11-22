/* format_test.c - the library's formatting against this machine's.
 *
 * Runs on the host, not in TheTaaJ. format.c is compiled for the host
 * with its functions renamed (taaj_snprintf for snprintf), and every
 * format below is given to both that and the host's own snprintf, with
 * the same arguments. The two must write the same bytes and return the
 * same number. The host's C library has had decades of people finding
 * its mistakes; agreeing with it is the test.
 *
 * Built and run by `make libc-test`. */

#include <limits.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int taaj_snprintf(char *to, size_t size, const char *format, ...);
int taaj_sprintf(char *to, const char *format, ...);

static int checks;
static int failures;

/* Both, into buffers of the same size, and compared. The size is a
 * parameter so that text being cut short is compared too. */
#define SAME_IN(size, ...)                                                             \
    do {                                                                               \
        char theirs[512], ours[512];                                                   \
        memset(theirs, '#', sizeof theirs);                                            \
        memset(ours, '#', sizeof ours);                                                \
        int their_count = snprintf(theirs, (size), __VA_ARGS__);                       \
        int our_count = taaj_snprintf(ours, (size), __VA_ARGS__);                      \
        checks++;                                                                      \
        /* Past the zero nothing may have been touched: the whole of   */              \
        /* both buffers is compared, not only the strings.             */              \
        if (their_count != our_count || memcmp(theirs, ours, sizeof theirs) != 0) {    \
            failures++;                                                                \
            theirs[sizeof theirs - 1] = ours[sizeof ours - 1] = '\0';                  \
            printf("line %d: %s\n  host: %d \"%s\"\n  ours: %d \"%s\"\n", __LINE__,   \
                   #__VA_ARGS__, their_count, (size) ? theirs : "", our_count,         \
                   (size) ? ours : "");                                                \
        }                                                                              \
    } while (0)

#define SAME(...) SAME_IN(400, __VA_ARGS__)

static void expect(const char *what, const char *ours, const char *wanted)
{
    checks++;
    if (strcmp(ours, wanted) != 0) {
        failures++;
        printf("%s: ours \"%s\", wanted \"%s\"\n", what, ours, wanted);
    }
}

int main(void)
{
    /* Plain text, and the percent sign. */
    SAME("%s", "");
    SAME("hello");
    SAME("100%%");
    SAME("%%%d%%", 5);
    SAME("a%sb%sc", "1", "2");

    /* Whole numbers, at the edges. */
    SAME("%d", 0);
    SAME("%d", 42);
    SAME("%d", -42);
    SAME("%i", 7);
    SAME("%d %d", INT_MAX, INT_MIN);
    SAME("%ld %ld", LONG_MAX, LONG_MIN);
    SAME("%lld %lld", LLONG_MAX, LLONG_MIN);
    SAME("%u %u", 0u, UINT_MAX);
    SAME("%lu", ULONG_MAX);
    SAME("%llu", ULLONG_MAX);
    SAME("%zu %zu", (size_t)0, SIZE_MAX);
    SAME("%zd", (ptrdiff_t)-5);
    SAME("%jd %ju", INTMAX_MIN, UINTMAX_MAX);
    SAME("%td", (ptrdiff_t)-123456789);
    /* The short lengths cut the argument back down. */
    SAME("%hhd %hhd", 127, 200);
    SAME("%hhu %hhu", 255, 256 + 7);
    SAME("%hd %hd", 32767, 40000);
    SAME("%hu", 70000);
    SAME("%hhx", -1);

    /* Hex and octal. */
    SAME("%x %X", 255, 255);
    SAME("%x", 0);
    SAME("%x", 0xdeadbeefu);
    SAME("%lx", 0xfeedfacecafebeefUL);
    SAME("%o", 8);
    SAME("%o", 0);
    SAME("%lo", ULONG_MAX);
    SAME("%#x %#X", 255, 255);
    SAME("%#x", 0);
    SAME("%#o", 8);
    SAME("%#o", 0);
    SAME("%#.0o", 0);
    SAME("%#.5o", 8);
    SAME("%#10x|", 255);
    SAME("%#010x|", 255);
    SAME("%-#10x|", 255);

    /* Width, to the right and to the left. */
    SAME("[%5d]", 42);
    SAME("[%-5d]", 42);
    SAME("[%5d]", -42);
    SAME("[%-5d]", -42);
    SAME("[%1d]", 12345);
    SAME("[%5u]", 7u);
    SAME("[%10s]", "hi");
    SAME("[%-10s]", "hi");
    SAME("[%2s]", "longer");
    SAME("[%5c]", 'x');
    SAME("[%-5c]", 'x');
    SAME("[%c]", 'x');
    SAME("[%40d]", 1);
    SAME("[%-40d]", 1);

    /* Zeros: after the sign, and not with - or a precision. */
    SAME("[%05d]", 42);
    SAME("[%05d]", -42);
    SAME("[%-05d]", 42);
    SAME("[%05x]", 255);
    SAME("[%08.3d]", 42);
    SAME("[%030d]", -1);

    /* Signs. */
    SAME("%+d %+d %+d", 5, -5, 0);
    SAME("% d|% d|% d", 5, -5, 0);
    SAME("%+ d", 5);
    SAME("[%+5d]", 5);
    SAME("[%+05d]", 5);
    SAME("[% 05d]", 5);
    SAME("[%-+5d]", 5);

    /* Precision: the least digits of a number. */
    SAME("%.3d", 5);
    SAME("%.3d", -5);
    SAME("%.3d", 12345);
    SAME("%.0d", 0);
    SAME("[%.0d]", 7);
    SAME("[%.d]", 0);
    SAME("[%5.0d]", 0);
    SAME("[%8.3d]", 5);
    SAME("[%-8.3d]", 5);
    SAME("[%+.3d]", 5);
    SAME("%.5x", 255);
    SAME("%#.5x", 255);
    SAME("%.0x", 0);
    SAME("%#.0x", 0);
    SAME("%.20lu", ULONG_MAX);
    SAME("%.30lu", ULONG_MAX);

    /* Precision: the most bytes of a string. */
    SAME("%.3s", "abcdef");
    SAME("%.0s|", "abcdef");
    SAME("%.10s", "abc");
    SAME("[%8.3s]", "abcdef");
    SAME("[%-8.3s]", "abcdef");
    {
        /* With a precision the string need not end in a zero. */
        char unended[4] = { 'w', 'x', 'y', 'z' };
        SAME("%.4s", unended);
        SAME("%.2s", unended);
    }

    /* Width and precision from the arguments. */
    SAME("[%*d]", 6, 42);
    SAME("[%*d]", -6, 42);
    SAME("[%.*d]", 4, 42);
    SAME("[%.*d]", -4, 42);
    SAME("[%*.*d]", 8, 4, 42);
    SAME("[%.*s]", 2, "abcdef");
    SAME("[%*s]", 5, "ab");
    SAME("[%-*s]", 5, "ab");

    /* Addresses that are not null (the host writes null its own way). */
    SAME("%p", (void *)0x1000);
    SAME("%p", (void *)0xffff800000001234UL);
    SAME("[%20p]", (void *)0x1000);
    SAME("[%-20p]", (void *)0x1000);

    /* Several, of different sizes, in one format: each must take
     * exactly its own argument, or all the later ones are wrong. */
    SAME("%d %s %c %x %ld %s", 1, "two", '3', 4, 5L, "six");
    SAME("%hhd %lld %hd %zu %c %s", 1, 2LL, 3, (size_t)4, '5', "6");
    SAME("%s=%d, %s=%#lx, %5.2s|%-6u|", "a", -1, "b", 255UL, "xyz", 9u);

    /* Text longer than the padding helper's run, and than most. */
    SAME("[%100d]", 7);
    SAME("[%-100s]", "left");
    SAME("[%0100d]", -7);
    SAME("%s%s%s%s", "0123456789012345678901234567890123456789", "0123456789012345678901234567890123456789",
         "0123456789012345678901234567890123456789", "0123456789012345678901234567890123456789");

    /* A buffer too small: cut short, ended in a zero, nothing written
     * past it, and the whole length returned all the same. */
    SAME_IN(8, "hello, world");
    SAME_IN(8, "%d", 123456789);
    SAME_IN(6, "hello");
    SAME_IN(5, "hello");
    SAME_IN(1, "hello");
    SAME_IN(0, "hello");
    SAME_IN(4, "[%10d]", 5);
    SAME_IN(3, "%s", "abcdef");
    {
        /* Size 0 and no buffer at all: only the length is wanted. */
        checks++;
        int their_count = snprintf(NULL, 0, "%d apples", 12);
        int our_count = taaj_snprintf(NULL, 0, "%d apples", 12);
        if (their_count != our_count) {
            failures++;
            printf("snprintf(NULL, 0): host %d, ours %d\n", their_count, our_count);
        }
    }
    {
        /* sprintf: the same text, into a buffer taken on trust. */
        char theirs[64], ours[64];
        checks++;
        int their_count = sprintf(theirs, "%d-%s-%x", 12, "ab", 255);
        int our_count = taaj_sprintf(ours, "%d-%s-%x", 12, "ab", 255);
        if (their_count != our_count || strcmp(theirs, ours) != 0) {
            failures++;
            printf("sprintf: host %d \"%s\", ours %d \"%s\"\n", their_count, theirs, our_count, ours);
        }
    }

    /* Floating point. Ties first: exactly halfway, to the even digit -
     * 0.125 is exact in binary, 0.15 is not (it is a little under). */
    SAME("%.2f %.2f %.2f %.0f %.0f %.0f %.1f", 0.125, 0.375, 0.15, 0.5, 1.5, 2.5, 0.25);
    SAME("%f %f %f %f", 0.0, -0.0, 1.0, -1.5);
    SAME("%.0f|%#.0f|%.0e|%#.0e|%g|%#g", 3.0, 3.0, 3.0, 3.0, 3.0, 3.0);
    SAME("%e %E %.3e %.10e %e", 12345.678, 0.000123, 9.9996, 1.0 / 3, 0.0);
    SAME("%g %g %g %g %g %g %g", 100000.0, 1000000.0, 0.0001, 0.00001, 123456789.0, 0.1, 1e-300);
    SAME("%G %.3g %.10g %.0g %#.3g %g", 1e-10, 3.14159, 2.0 / 3, 25.0, 1.0, 0.0);
    SAME("%.17g %.17g %.17g %.17g", 0.1, 1e23, 2.2250738585072011e-308, 1.7976931348623157e308);
    SAME("[%10.3f] [%-10.3f] [%010.3f] [%+f] [% f] [%+.2e] [%010g]", 3.14159, 3.14159, -3.14159, 2.0, 2.0, -1.0, 2.5);
    SAME("%f %F %e %E %g %G [%8f] [%-8f] [%08f]", 1.0 / 0.0, -1.0 / 0.0, 1.0 / 0.0, 1.0 / 0.0, -1.0 / 0.0, 1.0 / 0.0,
         1.0 / 0.0, -1.0 / 0.0, 1.0 / 0.0);
    SAME("%f %F %e %g", __builtin_nan(""), __builtin_nan(""), -__builtin_nan(""), __builtin_nan(""));
    SAME("%.0f", 1e308);
    SAME("%f", 1.7976931348623157e308);
    SAME("%.30f %.20e", 5e-324, 5e-324);
    SAME("%.1074f", 5e-324); /* every digit: cut at the buffer, the length compared */
    SAME("%.400e", 2.2250738585072014e-308);
    SAME("%.3f %.15f %.16e %.0f %.0f", 9.9995, 0.1, 0.3, 9007199254740993.0, 4503599627370497.5);
    SAME("%Lf %Le %Lg", 2.5L, 2.5L, 0.0001L);
    SAME("%5.1f|%-7.2e|%.3G|%s", 99.95, 1e100, 1e-5, "after");
    {
        /* A thousand doubles from all over the range - random bits, so
         * every exponent - each in six ways. */
        uint64_t state = 0x9E3779B97F4A7C15u;
        for (int i = 0; i < 1000; i++) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            double value;
            memcpy(&value, &state, sizeof value);
            if (value != value || value - value != 0)
                continue; /* NaN and infinity have had their turn */
            SAME("%.17g|%e|%.25e|%g|%.0e", value, value, value, value, value);
            if (value < 1e300 && value > -1e300)
                SAME("%.3f", value);
        }
    }

    /* Where this library says what it does and the host need not
     * agree: checked against what is written down, not the host. */
    {
        char ours[64];
        taaj_snprintf(ours, sizeof ours, "%s", (char *)NULL);
        expect("a null string", ours, "(null)");
        taaj_snprintf(ours, sizeof ours, "%p", NULL);
        expect("a null address", ours, "0x0");
        taaj_snprintf(ours, sizeof ours, "a %5k b %d", 7);
        expect("a conversion that is not one", ours, "a %5k b 7");
        taaj_snprintf(ours, sizeof ours, "end %-5");
        expect("a format that ends inside a conversion", ours, "end %-5");
    }

    if (failures != 0) {
        printf("format: %d of %d checks FAILED\n", failures, checks);
        return 1;
    }
    printf("format: %d checks, all the same as the host's printf\n", checks);
    return 0;
}
