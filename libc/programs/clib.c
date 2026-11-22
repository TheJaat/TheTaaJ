/* clib.c - checks of the parts of the C library a C compiler needs:
 * setjmp and longjmp, numbers from text (strtod and its family, with
 * cases that are hard to round right), ldexp, fabs, printf's floating
 * point, the long long conversions, strpbrk, the time and the calendar, getcwd, realpath,
 * fdopen, getenv and execvp - and of what C++'s library needs of it:
 * sscanf, asprintf, strftime and mktime, the exact parts of math.h,
 * aligned_alloc, the locale, constructors and atexit.
 *
 *   with files clib     every check; "library: N checks, all passed",
 *                       then, from the atexit handlers, "library: atexit
 *                       ran its handlers, newest first"
 *   clib assert         a false assert: the message, and exit code 134
 *
 * The files checks read `welcome` and `docs/`, which every test disk
 * has. */

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <locale.h>
#include <math.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static int checks;
static int failures;

static void check(int ok, const char *what)
{
    checks++;
    if (!ok) {
        failures++;
        printf("clib: FAILED: %s\n", what);
    }
}

static uint64_t bits(double value)
{
    uint64_t b;
    memcpy(&b, &value, sizeof b);
    return b;
}

static uint32_t float_bits(float value)
{
    uint32_t b;
    memcpy(&b, &value, sizeof b);
    return b;
}

/* ---- setjmp and longjmp ---- */

static jmp_buf place;
static int depth;

static void dive(int levels, int value)
{
    depth++;
    if (levels == 0)
        longjmp(place, value);
    dive(levels - 1, value);
    depth = -1000; /* never reached: the jump skips the way back */
}

/* Doubles kept in registers across the jump: on AArch64, in the
 * callee-saved d8 to d15, which longjmp must put back. */
static double __attribute__((noinline)) after_jump(double a, double b)
{
    return a * b + 1.0;
}

static void jumps(void)
{
    volatile int times = 0;
    volatile double kept = 2.5;
    int got = setjmp(place);
    times++;
    if (got == 0) {
        dive(20, 7);
    } else if (got == 7) {
        check(depth == 21, "longjmp left all the calls between");
        check(times == 2, "setjmp returned a second time");
        check(after_jump(kept, 4.0) == 11.0, "doubles after longjmp");
        depth = 0;
        dive(3, 0);
    } else {
        check(got == 1, "longjmp with 0 makes setjmp return 1");
    }
}

/* ---- numbers ---- */

static void numbers(void)
{
    char *end;
    check(bits(strtod("0.1", NULL)) == 0x3fb999999999999aULL, "strtod 0.1, correctly rounded");
    check(float_bits(strtof("0.1", NULL)) == 0x3dcccccdU, "strtof 0.1, correctly rounded");
    /* 2^53 + 1 lies exactly between two doubles: to the even one. */
    check(bits(strtod("9007199254740993", NULL)) == 0x4340000000000000ULL, "strtod rounds a tie to even");
    /* A famous hard case: the largest subnormal double. */
    check(bits(strtod("2.2250738585072011e-308", NULL)) == 0x000fffffffffffffULL, "strtod 2.2250738585072011e-308");
    check(bits(strtod("1.7976931348623157e308", NULL)) == 0x7fefffffffffffffULL, "strtod the largest double");
    check(strtod("  -12.5e1xyz", &end) == -125.0 && strcmp(end, "xyz") == 0, "strtod spaces, sign, exponent, end");
    check(strtod(".5", NULL) == 0.5 && strtod("5.", NULL) == 5.0, "strtod .5 and 5.");
    check(strtod("7e", &end) == 7.0 && strcmp(end, "e") == 0, "strtod leaves an e with no exponent");
    end = NULL;
    const char *nothing = "abc";
    check(strtod(nothing, &end) == 0.0 && end == nothing, "strtod with no number");
    errno = 0;
    double huge = strtod("1e999", NULL);
    check(huge > 1e308 && huge - huge != 0 && errno == ERANGE, "strtod overflow: infinity, ERANGE");
    errno = 0;
    check(strtod("1e-999", NULL) == 0.0 && errno == ERANGE, "strtod underflow: 0, ERANGE");
    errno = 0;
    double inf = strtod("-Infinity", NULL);
    check(inf < -1e308 && errno == 0, "strtod -Infinity, no ERANGE");
    double nan = strtod("nan", NULL);
    check(nan != nan, "strtod nan");
    check(strtold("2.5", NULL) == 2.5L, "strtold");
    check(atof("3.25") == 3.25, "atof");

    check(ldexp(1.0, 10) == 1024.0, "ldexp up");
    check(ldexp(3.0, -2) == 0.75, "ldexp down");
    check(bits(ldexp(1.0, -1074)) == 1, "ldexp to the smallest double");
    check(bits(ldexp(1.0, -1075)) == 0, "ldexp below it: 0");
    check(ldexp(1.0, 1024) > 1e308, "ldexp past the largest: infinity");
    check(ldexp(0x1p-1074, 2000) == 0x1p926, "ldexp in steps that each fit");
    check(ldexpl(1.0L, 100) == 0x1p100L, "ldexpl");
    check(ldexpf(1.5f, 3) == 12.0f, "ldexpf");
    check(fabs(-2.0) == 2.0, "fabs");
    /* In parentheses: the C library's own, not the compiler's built-in
     * - the ones a compiler without the built-in (TCC) calls. */
    check((fabs)(-2.0) == 2.0 && bits((fabs)(-0.0)) == 0, "the library's fabs, and of -0.0");
    check((fabsf)(-1.5f) == 1.5f && (fabsl)(-3.0L) == 3.0L && (fabsl)(4.0L) == 4.0L, "the library's fabsf and fabsl");

    check(strtoll("-9223372036854775808", NULL, 10) == LLONG_MIN, "strtoll the most negative");
    check(strtoull("18446744073709551615", NULL, 10) == ULLONG_MAX, "strtoull the largest");
    check(strtoull("0x10", NULL, 0) == 16, "strtoull base 0");

    char text[80];
    snprintf(text, sizeof text, "%.2f %.3e %g %g %.17g %5.1f|%Lg", 0.125, 12345.678, 1e-5, 100000.0, 0.1, -0.04, 2.5L);
    check(strcmp(text, "0.12 1.235e+04 1e-05 100000 0.10000000000000001  -0.0|2.5") == 0, "printf's floating point");

    const char *s = "a=b+c";
    check(strpbrk(s, "+=") == s + 1 && strpbrk(s, "xyz") == NULL, "strpbrk");
}

/* ---- the time ---- */

static int is_date(time_t when, int year, int mon, int mday, int hour, int min, int sec, int wday, int yday)
{
    struct tm *t = gmtime(&when);
    return t->tm_year == year && t->tm_mon == mon && t->tm_mday == mday && t->tm_hour == hour && t->tm_min == min
        && t->tm_sec == sec && t->tm_wday == wday && t->tm_yday == yday && t->tm_isdst == 0;
}

static void times(void)
{
    time_t first = time(NULL);
    time_t stored = -1;
    check(first >= 0 && time(&stored) >= first && stored >= first, "time");
    struct timeval tv;
    check(gettimeofday(&tv, NULL) == 0 && tv.tv_sec >= first && tv.tv_usec >= 0 && tv.tv_usec < 1000000, "gettimeofday");
    check(is_date(0, 70, 0, 1, 0, 0, 0, 4, 0), "gmtime of 0: Thursday 1 January 1970");
    check(is_date(951782400, 100, 1, 29, 0, 0, 0, 2, 59), "gmtime of 29 February 2000");
    check(is_date(68169600, 72, 1, 29, 0, 0, 0, 2, 59), "gmtime of 29 February 1972");
    check(is_date(4102444799, 199, 11, 31, 23, 59, 59, 4, 364), "gmtime of the last second of 2099");
    check(is_date(-1, 69, 11, 31, 23, 59, 59, 3, 364), "gmtime of the second before 1970");
    struct tm out;
    time_t zero = 0;
    check(localtime_r(&zero, &out) == &out && out.tm_year == 70, "localtime_r");
}

/* ---- files and the process ---- */

static void files(void)
{
    char where[PATH_MAX];
    check(getcwd(where, sizeof where) == where && strcmp(where, "/") == 0, "getcwd: started at the root");
    errno = 0;
    check(getcwd(where, 1) == NULL && errno == ERANGE, "getcwd with too little room");
    char *mine = getcwd(NULL, 0);
    check(mine != NULL && strcmp(mine, "/") == 0, "getcwd into malloc's memory");
    free(mine);

    char *path = realpath("docs/../welcome", NULL);
    check(path != NULL && strcmp(path, "/welcome") == 0, "realpath works out ..");
    free(path);
    check(realpath("docs", where) == where && strcmp(where, "/docs") == 0, "realpath of a directory");
    errno = 0;
    check(realpath("nofile", where) == NULL && errno == ENOENT, "realpath of nothing: ENOENT");

    int descriptor = open("welcome", O_RDONLY);
    FILE *stream = descriptor >= 0 ? fdopen(descriptor, "r") : NULL;
    char line[64] = "";
    check(stream != NULL && fgets(line, sizeof line, stream) != NULL && strcmp(line, "Hello from the host\n") == 0,
          "fdopen and read a line");
    check(stream != NULL && fclose(stream) == 0, "fclose a stream fdopen made");

    check(getenv("PATH") == NULL, "getenv: no environment");
    char *arguments[] = {"ls", NULL};
    errno = 0;
    check(execvp("ls", arguments) == -1 && errno == ENOSYS, "execvp: ENOSYS");
}

/* ---- what C++'s library needs ---- */

static int constructed;
__attribute__((constructor)) static void before_main(void)
{
    constructed = 1;
}

/* Run by exit, newest first: the second registered prints the start of
 * the line, the first its end. */
static void handler_first(void)
{
    printf("newest first\n");
}

static void handler_second(void)
{
    printf("library: atexit ran its handlers, ");
}

static void for_cxx(void)
{
    check(constructed == 1, "a constructor ran before main");
    check(atexit(handler_first) == 0 && atexit(handler_second) == 0, "atexit");

    int n = 0, got;
    char word[16];
    double weight = 0;
    unsigned hex = 0;
    got = sscanf("12 apples 3.5 ff", "%d %15s %lf %x", &n, word, &weight, &hex);
    check(got == 4 && n == 12 && strcmp(word, "apples") == 0 && weight == 3.5 && hex == 255, "sscanf");
    long double big = 0;
    char rest[8];
    /* 1e300 would not do: strtold goes through a double, which is
     * not the nearest long double to it - and TCC, parsing a 1e300L
     * here with the same strtold, would then write this file differently
     * on TheTaaJ than on the host. 2^-20 is exact in both. */
    got = sscanf("x=9.5367431640625e-7,abc", "x=%Lf,%[a-c]", &big, rest);
    check(got == 2 && big == 0x1p-20L && strcmp(rest, "abc") == 0, "sscanf: %Lf and %[");
    check(sscanf("", "%d", &n) == EOF && sscanf("z", "%d", &n) == 0, "sscanf: nothing, and no match");

    char *text = NULL;
    check(asprintf(&text, "%s %d %.2f", "pi", 3, 3.14159) == 9 && strcmp(text, "pi 3 3.14") == 0, "asprintf");
    free(text);

    struct tm t = {0};
    t.tm_year = 126; /* 2026 */
    t.tm_mon = 10;   /* November */
    t.tm_mday = 22;
    t.tm_hour = 17;
    t.tm_min = 5;
    time_t when = mktime(&t);
    check(when == 1795367100 && t.tm_wday == 0 && t.tm_yday == 325, "mktime");
    char date[64];
    check(strftime(date, sizeof date, "%A %d %B %Y, %H:%M, week %V, day %j", &t) > 0
              && strcmp(date, "Sunday 22 November 2026, 17:05, week 47, day 326") == 0,
          "strftime");
    t.tm_mday = 32; /* 32 November is 2 December */
    mktime(&t);
    check(t.tm_mon == 11 && t.tm_mday == 2, "mktime puts days out of range right");

    check(sqrt(2.0) == 1.4142135623730951 && sqrt(-1.0) != sqrt(-1.0), "sqrt");
    check(floor(-2.5) == -3.0 && ceil(-2.5) == -2.0 && round(-2.5) == -3.0 && rint(2.5) == 2.0, "rounding");
    check(fmod(7.5, 2.0) == 1.5 && remainder(7.5, 2.0) == -0.5, "fmod and remainder");
    int exponent;
    check(frexp(48.0, &exponent) == 0.75 && exponent == 6, "frexp");
    check(isnan(nan("")) && isinf(HUGE_VAL) && fpclassify(1e-310) == FP_SUBNORMAL && signbit(-0.0), "classifying");

    void *aligned = aligned_alloc(256, 100);
    check(aligned != NULL && ((uintptr_t)aligned & 255) == 0, "aligned_alloc");
    free(aligned);

    check(strcmp(setlocale(LC_ALL, ""), "C") == 0 && setlocale(LC_ALL, "fr_FR") == NULL, "setlocale");
    check(strcmp(localeconv()->decimal_point, ".") == 0 && strcoll("a", "b") < 0, "localeconv and strcoll");
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "assert") == 0) {
        int number = 1;
        assert(number == 2);
        return 0; /* never: the assert ends the program */
    }
    jumps();
    numbers();
    times();
    files();
    for_cxx();
    if (failures == 0)
        printf("library: %d checks, all passed\n", checks);
    else
        printf("library: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
