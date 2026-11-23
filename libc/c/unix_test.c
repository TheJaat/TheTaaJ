/* unix_test.c - unix.c's basename, dirname, fnmatch, getopt and the
 * environment, against this machine's.
 *
 * Runs on the host, not in TheTaaJ. unix.c is compiled for the host as
 * it is, and the names it defines changed in the object file (objcopy:
 * taaj_fnmatch for fnmatch), so that it sits in one program with the
 * host's own. glob is not asked here: it reads directories through
 * TheTaaJ's dirent and stat, which are not the host's (clib.c checks it
 * on TheTaaJ).
 *
 *   basename, dirname   awkward paths: slashes doubled, at the ends,
 *                       alone; the same answer as the host's
 *   fnmatch             patterns and names chosen to be awkward, with
 *                       each set of flags: the same answer
 *   getopt              option lists, grouped, with arguments joined
 *                       and apart, unknown, missing, after "--", and
 *                       started again with optind = 0
 *   setenv and the rest set, replaced, kept, removed, and listed
 *
 * Built and run by `make libc-test`. */

#define _GNU_SOURCE
#include <fnmatch.h>
#include <libgen.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int taaj_errno;

char *taaj_basename(char *path);
char *taaj_dirname(char *path);
int taaj_fnmatch(const char *pattern, const char *string, int flags);
int taaj_getopt(int argc, char *const argv[], const char *optstring);
extern char *taaj_optarg;
extern int taaj_optind, taaj_opterr, taaj_optopt;
char *taaj_getenv(const char *name);
int taaj_setenv(const char *name, const char *value, int overwrite);
int taaj_unsetenv(const char *name);
int taaj_putenv(char *entry);
extern char **taaj_environ;

/* popen's other half is spawn.rs, which is not here: popen is not asked
 * (pipes.c checks it on TheTaaJ), only linked. */
int __taaj_popen(const char *command, int writing)
{
    (void)command;
    (void)writing;
    return -1;
}

int __taaj_pclose(int fd)
{
    (void)fd;
    return -1;
}

/* TheTaaJ's fnmatch.h values: the host's may differ. */
#define T_NOESCAPE 0x01
#define T_PATHNAME 0x02
#define T_PERIOD 0x04

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        fprintf(stderr, "unix: failed: %s\n", what);
    }
}

static void paths(void)
{
    const char *cases[] = {"", "/", "//", "a", "a/", "a//", "/a", "/a/", "a/b", "a/b/", "a//b", "/a/b",
                           "/a/b/", "./a", "../a/b", "a/./b", ".", "..", "/.", "a/b/c/d", "///a///b///"};
    for (size_t n = 0; n < sizeof cases / sizeof *cases; n++) {
        char mine[64], theirs[64], what[128];
        strcpy(mine, cases[n]);
        strcpy(theirs, cases[n]);
        snprintf(what, sizeof what, "basename(\"%s\")", cases[n]);
        check(strcmp(taaj_basename(mine), __xpg_basename(theirs)) == 0, what);
        strcpy(mine, cases[n]);
        strcpy(theirs, cases[n]);
        snprintf(what, sizeof what, "dirname(\"%s\")", cases[n]);
        /* "//" may be "/" or "//", as POSIX allows: glibc says "//". */
        const char *got = taaj_dirname(mine);
        check(strcmp(got, dirname(theirs)) == 0 || (strcmp(cases[n], "//") == 0 && strcmp(got, "/") == 0), what);
    }
    check(strcmp(taaj_basename(NULL), ".") == 0 && strcmp(taaj_dirname(NULL), ".") == 0, "NULL is \".\"");
}

static void patterns(void)
{
    const char *list[][2] = {
        {"*", "anything"}, {"*", ""}, {"*", ".hidden"}, {".*", ".hidden"}, {"*.c", "main.c"}, {"*.c", "main.h"},
        {"*.c", ".c"}, {"?", "a"}, {"?", ""}, {"?", "ab"}, {"?", "."}, {"a?c", "abc"}, {"a*b*c", "aXbYc"},
        {"a*b*c", "aXbY"}, {"*a*", "banana"}, {"**", "x"}, {"[abc]", "b"}, {"[abc]", "d"}, {"[a-c]x", "bx"},
        {"[!a-c]", "d"}, {"[!a-c]", "b"}, {"[^a]", "b"}, {"[]]", "]"}, {"[]a]", "a"}, {"[!]]", "x"},
        {"[a-]", "-"}, {"[", "["}, {"[ab", "[ab"}, {"a[", "a["}, {"\\*", "*"}, {"\\*", "x"}, {"a\\", "a\\"},
        {"\\[a]", "[a]"}, {"[\\]]", "]"}, {"*/*", "a/b"}, {"*", "a/b"}, {"a/*", "a/.b"}, {"a?b", "a/b"},
        {"a[/]b", "a/b"}, {"src/*.c", "src/x.c"}, {"*.[ch]", "x.h"}, {"*.[ch]", "x.o"}, {"x*y*z", "xyz"},
        {"*x", "xxxx"}, {"*?*", "ab"}, {"a*", "a"},
    };
    int flags[] = {0, T_PATHNAME, T_PERIOD, T_PATHNAME | T_PERIOD, T_NOESCAPE};
    int host[] = {0, FNM_PATHNAME, FNM_PERIOD, FNM_PATHNAME | FNM_PERIOD, FNM_NOESCAPE};
    for (size_t n = 0; n < sizeof list / sizeof *list; n++) {
        for (size_t f = 0; f < sizeof flags / sizeof *flags; f++) {
            char what[128];
            snprintf(what, sizeof what, "fnmatch(\"%s\", \"%s\", flags %d)", list[n][0], list[n][1], flags[f]);
            int mine = taaj_fnmatch(list[n][0], list[n][1], flags[f]);
            int theirs = fnmatch(list[n][0], list[n][1], host[f]);
            check((mine == 0) == (theirs == 0), what);
        }
    }
}

/* Runs taaj_getopt over words, writing what it says: "a", "b=x", "?c",
 * ":c", then "|" and the words left. */
static void options(const char *optstring, char **words, char *out)
{
    int argc = 0;
    while (words[argc] != NULL)
        argc++;
    *out = '\0';
    int c;
    taaj_opterr = 0;
    while ((c = taaj_getopt(argc, words, optstring)) != -1) {
        char one[32];
        if (c == '?' || c == ':')
            snprintf(one, sizeof one, "%c%c ", c, taaj_optopt);
        else if (strchr(optstring, c) && strchr(optstring, c)[1] == ':')
            snprintf(one, sizeof one, "%c=%s ", c, taaj_optarg);
        else
            snprintf(one, sizeof one, "%c ", c);
        strcat(out, one);
    }
    strcat(out, "|");
    for (int n = taaj_optind; n < argc; n++) {
        strcat(out, " ");
        strcat(out, words[n]);
    }
}

static void getopts(void)
{
    struct {
        const char *optstring;
        char *words[8];
        const char *expected;
    } cases[] = {
        {"ab", {"p", "-a", "-b", "x", NULL}, "a b | x"},
        {"ab", {"p", "-ab", "x", NULL}, "a b | x"},
        {"o:", {"p", "-o", "out", "in", NULL}, "o=out | in"},
        {"o:", {"p", "-oout", "in", NULL}, "o=out | in"},
        {"ao:", {"p", "-aofile", NULL}, "a o=file |"},
        {"a", {"p", "-x", "-a", NULL}, "?x a |"},
        {"o:", {"p", "-o", NULL}, "?o |"},
        {":o:", {"p", "-o", NULL}, ":o |"},
        {"a", {"p", "-a", "--", "-a", NULL}, "a | -a"},
        {"a", {"p", "x", "-a", NULL}, "| x -a"},
        {"a", {"p", "-", "-a", NULL}, "| - -a"},
        {"+ehij:k", {"p", "-e", "-j4", "-k", "all", NULL}, "e j=4 k | all"},
        {"a", {"p", NULL}, "|"},
    };
    for (size_t n = 0; n < sizeof cases / sizeof *cases; n++) {
        char out[256];
        taaj_optind = 0;
        options(cases[n].optstring, cases[n].words, out);
        char what[300];
        snprintf(what, sizeof what, "getopt case %zu: \"%s\", expected \"%s\"", n, out, cases[n].expected);
        check(strcmp(out, cases[n].expected) == 0, what);
    }
    /* Started again partway: optind = 0, as make does between MAKEFLAGS
     * and the command line. */
    char *first[] = {"p", "-ab", "rest", NULL};
    char *second[] = {"p", "-a", NULL};
    char out[256];
    taaj_optind = 0;
    taaj_getopt(3, first, "ab");
    taaj_optind = 0;
    options("ab", second, out);
    check(strcmp(out, "a |") == 0, "optind = 0 starts again, even in the middle of \"-ab\"");
}

static int listed(const char *entry)
{
    for (char **e = taaj_environ; *e != NULL; e++)
        if (strcmp(*e, entry) == 0)
            return 1;
    return 0;
}

static void environment(void)
{
    check(taaj_environ != NULL && taaj_environ[0] == NULL, "it starts empty");
    check(taaj_getenv("HOME") == NULL, "nothing is set");
    check(taaj_setenv("CC", "clang", 1) == 0 && strcmp(taaj_getenv("CC"), "clang") == 0, "setenv, getenv");
    check(taaj_setenv("CC", "tcc", 0) == 0 && strcmp(taaj_getenv("CC"), "clang") == 0, "overwrite 0 keeps it");
    check(taaj_setenv("CC", "tcc", 1) == 0 && strcmp(taaj_getenv("CC"), "tcc") == 0, "overwrite 1 replaces it");
    check(taaj_setenv("C", "short", 1) == 0 && strcmp(taaj_getenv("C"), "short") == 0 && strcmp(taaj_getenv("CC"), "tcc") == 0,
          "a name that is the start of another is not it");
    check(taaj_setenv("EMPTY", "", 1) == 0 && taaj_getenv("EMPTY") != NULL && *taaj_getenv("EMPTY") == '\0', "an empty value is set");
    for (int n = 0; n < 50; n++) {
        char name[16];
        snprintf(name, sizeof name, "V%d", n);
        taaj_setenv(name, name, 1);
    }
    check(strcmp(taaj_getenv("V49"), "V49") == 0 && strcmp(taaj_getenv("V0"), "V0") == 0, "fifty more");
    check(listed("CC=tcc") && listed("V7=V7") && !listed("CC=clang"), "environ lists them");
    static char entry[] = "MINE=one";
    check(taaj_putenv(entry) == 0 && strcmp(taaj_getenv("MINE"), "one") == 0, "putenv");
    entry[5] = 'X';
    check(strcmp(taaj_getenv("MINE"), "Xne") == 0, "putenv keeps the string itself");
    check(taaj_unsetenv("CC") == 0 && taaj_getenv("CC") == NULL && strcmp(taaj_getenv("C"), "short") == 0, "unsetenv");
    check(taaj_unsetenv("CC") == 0, "unsetenv of nothing");
    check(taaj_setenv("A=B", "x", 1) == -1 && taaj_setenv("", "x", 1) == -1 && taaj_unsetenv("") == -1, "bad names refused");
    check(taaj_putenv((char *)"NOEQUALS") == -1, "putenv without '='");
    int count = 0;
    for (char **e = taaj_environ; *e != NULL; e++)
        count++;
    check(count == 53, "and the list is the right length");
}

int main(void)
{
    paths();
    patterns();
    getopts();
    environment();
    if (failures) {
        printf("unix: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("unix: %d checks, all the same as the host's or as POSIX says\n", checks);
    return 0;
}
