/* unix.c - the parts of a Unix C library that make needs and that are
 * plain C over the rest of this one: basename and dirname (libgen.h),
 * getopt (unistd.h), the environment (stdlib.h), fnmatch (fnmatch.h)
 * and glob (glob.h); and popen and pclose (stdio.h), around spawn.rs. */

#include <dirent.h>
#include <errno.h>
#include <fnmatch.h>
#include <glob.h>
#include <libgen.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <stdarg.h>
#include <unistd.h>

/* ---- libgen.h ------------------------------------------------------ */

/* The last name in a path, trailing slashes dropped: "a/b/" is "b",
 * "/" is "/", "" or NULL is ".". Changes the path in place, as POSIX
 * allows. */
char *basename(char *path)
{
    static char dot[] = ".";
    if (path == NULL || *path == '\0')
        return dot;
    size_t end = strlen(path);
    while (end > 1 && path[end - 1] == '/')
        path[--end] = '\0';
    if (end == 1 && path[0] == '/')
        return path;
    char *last = strrchr(path, '/');
    return last ? last + 1 : path;
}

/* All but the last name: "a/b" is "a", "/a" is "/", "a" is ".". In
 * place, too. */
char *dirname(char *path)
{
    static char dot[] = ".";
    static char root[] = "/";
    if (path == NULL || *path == '\0')
        return dot;
    size_t end = strlen(path);
    while (end > 1 && path[end - 1] == '/')
        end--;
    while (end > 0 && path[end - 1] != '/')
        end--;
    if (end == 0)
        return dot;
    while (end > 1 && path[end - 1] == '/')
        end--;
    if (end == 1 && path[0] == '/')
        return root;
    path[end] = '\0';
    return path;
}

/* ---- getopt -------------------------------------------------------- */

char *optarg;
int optind = 1, opterr = 1, optopt;
/* Where in argv[optind] the next option letter is: options can be
 * grouped, "-ab". */
static int letter = 1;

/* The options in argv, one per call, as POSIX says: it stops at the
 * first word that is not an option, or at "--". An optstring starting
 * with ':' reports a missing argument as ':' and prints nothing; a '+'
 * at the start (what GNU's getopt is told not to reorder by) is skipped:
 * this one never reorders. optind = 0 starts again from the first
 * argument, as GNU's does. */
int getopt(int argc, char *const argv[], const char *optstring)
{
    if (optind == 0) {
        optind = 1;
        letter = 1;
    }
    if (*optstring == '+')
        optstring++;
    int quiet = *optstring == ':';
    if (optind >= argc || argv[optind] == NULL || argv[optind][0] != '-' || argv[optind][1] == '\0')
        return -1;
    if (strcmp(argv[optind], "--") == 0) {
        optind++;
        return -1;
    }
    char c = argv[optind][letter];
    const char *at = c == ':' ? NULL : strchr(optstring, c);
    optopt = c;
    if (argv[optind][++letter] == '\0') {
        optind++;
        letter = 1;
    }
    if (at == NULL) {
        if (opterr && !quiet)
            fprintf(stderr, "%s: unknown option -%c\n", argv[0], c);
        return '?';
    }
    if (at[1] == ':') {
        if (letter > 1) {
            /* The rest of this word is the argument: "-ofile". */
            optarg = &argv[optind][letter];
            optind++;
            letter = 1;
        } else if (optind < argc) {
            optarg = argv[optind++];
        } else {
            if (opterr && !quiet)
                fprintf(stderr, "%s: option -%c needs an argument\n", argv[0], c);
            return quiet ? ':' : '?';
        }
    }
    return c;
}

/* ---- the environment ----------------------------------------------- */

/* A program starts with what the program that started it passed
 * (posix_spawn's envp, or environ for system and popen: libc/src/
 * spawn.rs); one the shell started, with none. The library's start
 * (libc/src/lib.rs) points environ at an array of its own before main.
 * What a program sets, it finds again, and passes on. */
static char *empty[] = {NULL};
char **environ = empty;
/* Whether environ is this library's own array, made with malloc. */
static int owned;
static size_t count;

/* The entry for name, as an index into environ, or -1. */
static long find(const char *name, size_t length)
{
    for (size_t n = 0; environ[n] != NULL; n++)
        if (strncmp(environ[n], name, length) == 0 && environ[n][length] == '=')
            return (long)n;
    return -1;
}

char *getenv(const char *name)
{
    size_t length = strlen(name);
    long at = find(name, length);
    return at < 0 ? NULL : environ[at] + length + 1;
}

/* Puts entry ("NAME=value") in, in place of the one for the same name,
 * keeping environ's array as this library's own. */
static int put_entry(char *entry, size_t name_length)
{
    long at = find(entry, name_length);
    if (at >= 0) {
        environ[at] = entry;
        return 0;
    }
    if (!owned) {
        size_t n = 0;
        while (environ[n] != NULL)
            n++;
        char **copy = malloc((n + 1) * sizeof *copy);
        if (copy == NULL)
            return -1;
        memcpy(copy, environ, (n + 1) * sizeof *copy);
        environ = copy;
        count = n;
        owned = 1;
    }
    char **grown = realloc(environ, (count + 2) * sizeof *grown);
    if (grown == NULL)
        return -1;
    environ = grown;
    environ[count++] = entry;
    environ[count] = NULL;
    return 0;
}

static int bad_name(const char *name)
{
    if (name == NULL || *name == '\0' || strchr(name, '=') != NULL) {
        errno = EINVAL;
        return 1;
    }
    return 0;
}

/* Sets name to value; with overwrite 0, only if it is not set. The
 * strings are copied. (An entry replaced is not freed: getenv may have
 * handed it out.) */
int setenv(const char *name, const char *value, int overwrite)
{
    if (bad_name(name))
        return -1;
    size_t length = strlen(name);
    if (!overwrite && find(name, length) >= 0)
        return 0;
    char *entry = malloc(length + strlen(value) + 2);
    if (entry == NULL) {
        errno = ENOMEM;
        return -1;
    }
    memcpy(entry, name, length);
    entry[length] = '=';
    strcpy(entry + length + 1, value);
    if (put_entry(entry, length) != 0) {
        free(entry);
        errno = ENOMEM;
        return -1;
    }
    return 0;
}

int unsetenv(const char *name)
{
    if (bad_name(name))
        return -1;
    long at;
    while ((at = find(name, strlen(name))) >= 0) {
        size_t n = (size_t)at;
        do {
            environ[n] = environ[n + 1];
        } while (environ[n++] != NULL);
        if (owned)
            count--;
    }
    return 0;
}

/* Puts "NAME=value" itself in, not a copy: changing the string later
 * changes the variable, as POSIX says. */
int putenv(char *entry)
{
    char *equals = strchr(entry, '=');
    if (equals == NULL || equals == entry) {
        errno = EINVAL;
        return -1;
    }
    if (put_entry(entry, (size_t)(equals - entry)) != 0) {
        errno = ENOMEM;
        return -1;
    }
    return 0;
}

/* ---- fnmatch -------------------------------------------------------- */

/* Whether c is in the bracket expression at *pattern (just past '['),
 * and where it ends: *pattern is left past its ']'. -1 if it does not
 * end - then '[' is taken as itself. */
static int bracket(const char **pattern, char c, int flags)
{
    const char *p = *pattern;
    int negate = *p == '!' || *p == '^';
    if (negate)
        p++;
    int found = 0;
    int first = 1;
    while (*p != ']' || first) {
        if (*p == '\0')
            return -1;
        char low = *p++;
        if (low == '\\' && !(flags & FNM_NOESCAPE) && *p != '\0')
            low = *p++;
        char high = low;
        if (*p == '-' && p[1] != ']' && p[1] != '\0') {
            high = p[1];
            p += 2;
            if (high == '\\' && !(flags & FNM_NOESCAPE) && *p != '\0')
                high = *p++;
        }
        if (c >= low && c <= high)
            found = 1;
        first = 0;
    }
    *pattern = p + 1;
    return found != negate;
}

static int match(const char *pattern, const char *string, const char *start, int flags)
{
    for (;;) {
        char p = *pattern++;
        char c = *string;
        /* A leading period (of the name, or after a slash with
         * FNM_PATHNAME) matches only a period in the pattern. */
        int leading_period = c == '.' && (flags & FNM_PERIOD) &&
                             (string == start || ((flags & FNM_PATHNAME) && string[-1] == '/'));
        switch (p) {
        case '\0':
            return c == '\0' ? 0 : FNM_NOMATCH;
        case '?':
            if (c == '\0' || leading_period || ((flags & FNM_PATHNAME) && c == '/'))
                return FNM_NOMATCH;
            string++;
            break;
        case '*':
            if (leading_period)
                return FNM_NOMATCH;
            while (*pattern == '*')
                pattern++;
            for (const char *s = string;; s++) {
                if (match(pattern, s, start, flags) == 0)
                    return 0;
                if (*s == '\0' || ((flags & FNM_PATHNAME) && *s == '/'))
                    return FNM_NOMATCH;
            }
        case '[': {
            if (c == '\0' || leading_period || ((flags & FNM_PATHNAME) && c == '/'))
                return FNM_NOMATCH;
            const char *after = pattern;
            int in = bracket(&after, c, flags);
            if (in < 0) {
                if (c != '[')
                    return FNM_NOMATCH;
            } else {
                if (!in)
                    return FNM_NOMATCH;
                pattern = after;
            }
            string++;
            break;
        }
        case '\\':
            if (!(flags & FNM_NOESCAPE)) {
                /* A backslash at the end escapes nothing: no match, as
                 * glibc and musl say (POSIX leaves it open). */
                if (*pattern == '\0')
                    return FNM_NOMATCH;
                p = *pattern++;
            }
            /* fall through */
        default:
            if (c != p)
                return FNM_NOMATCH;
            string++;
            break;
        }
    }
}

/* Whether string matches the shell wildcard pattern: 0, or FNM_NOMATCH.
 * FNM_PATHNAME, FNM_PERIOD and FNM_NOESCAPE as POSIX says; no
 * character classes ("[[:digit:]]"). */
int fnmatch(const char *pattern, const char *string, int flags)
{
    return match(pattern, string, string, flags);
}

/* ---- glob ------------------------------------------------------------ */

static int has_wildcard(const char *s, size_t length, int flags)
{
    for (size_t n = 0; n < length; n++) {
        if (s[n] == '\\' && !(flags & GLOB_NOESCAPE)) {
            n++;
        } else if (s[n] == '*' || s[n] == '?' || s[n] == '[') {
            return 1;
        }
    }
    return 0;
}

static int add_path(glob_t *found, const char *path)
{
    char **grown = realloc(found->gl_pathv, (found->gl_offs + found->gl_pathc + 2) * sizeof *grown);
    if (grown == NULL)
        return GLOB_NOSPACE;
    found->gl_pathv = grown;
    char *copy = strdup(path);
    if (copy == NULL)
        return GLOB_NOSPACE;
    grown[found->gl_offs + found->gl_pathc++] = copy;
    grown[found->gl_offs + found->gl_pathc] = NULL;
    return 0;
}

/* The paths matching rest (what is left of the pattern), in the
 * directory prefix (which ends in '/', or is ""). */
static int expand(const char *prefix, const char *rest, int flags, glob_t *found)
{
    while (*rest == '/')
        rest++;
    const char *slash = strchr(rest, '/');
    size_t length = slash ? (size_t)(slash - rest) : strlen(rest);
    char *name = malloc(length + 1);
    if (name == NULL)
        return GLOB_NOSPACE;
    memcpy(name, rest, length);
    name[length] = '\0';
    size_t room = strlen(prefix) + 300;
    char *path = malloc(room + length);
    if (path == NULL) {
        free(name);
        return GLOB_NOSPACE;
    }
    int result = 0;
    if (!has_wildcard(rest, length, flags)) {
        /* A plain name: there or not, without reading the directory
         * (backslashes taken out). */
        char *s = name, *d = name;
        for (; *s; s++) {
            if (*s == '\\' && !(flags & GLOB_NOESCAPE) && s[1] != '\0')
                s++;
            *d++ = *s;
        }
        *d = '\0';
        snprintf(path, room + length, "%s%s", prefix, name);
        struct stat status;
        if (stat(path, &status) == 0) {
            if (slash == NULL) {
                if ((flags & GLOB_MARK) && S_ISDIR(status.st_mode))
                    strcat(path, "/");
                result = add_path(found, path);
            } else if (S_ISDIR(status.st_mode)) {
                strcat(path, "/");
                result = expand(path, slash, flags, found);
            }
        }
    } else {
        DIR *dir = opendir(*prefix ? prefix : ".");
        if (dir == NULL) {
            result = (flags & GLOB_ERR) ? GLOB_ABORTED : 0;
        } else {
            int match_flags = FNM_PERIOD | ((flags & GLOB_NOESCAPE) ? FNM_NOESCAPE : 0);
            for (struct dirent *entry; result == 0 && (entry = readdir(dir)) != NULL;) {
                if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0)
                    continue;
                if (fnmatch(name, entry->d_name, match_flags) != 0)
                    continue;
                snprintf(path, room + length, "%s%s", prefix, entry->d_name);
                int is_dir = entry->d_type == DT_DIR;
                if (slash == NULL) {
                    if ((flags & GLOB_MARK) && is_dir)
                        strcat(path, "/");
                    result = add_path(found, path);
                } else if (is_dir) {
                    strcat(path, "/");
                    result = expand(path, slash, flags, found);
                }
            }
            closedir(dir);
        }
    }
    free(path);
    free(name);
    return result;
}

static int by_name(const void *a, const void *b)
{
    return strcmp(*(char *const *)a, *(char *const *)b);
}

/* The paths that match pattern, into *found: gl_pathc of them in
 * gl_pathv (after gl_offs nulls with GLOB_DOOFFS), sorted unless
 * GLOB_NOSORT. 0, or GLOB_NOMATCH (with GLOB_NOCHECK: the pattern
 * itself instead), GLOB_NOSPACE or GLOB_ABORTED. GLOB_ERR, GLOB_MARK,
 * GLOB_APPEND and GLOB_NOESCAPE as POSIX says; errfunc is not called. */
int glob(const char *pattern, int flags, int (*errfunc)(const char *, int), glob_t *found)
{
    (void)errfunc;
    if (!(flags & GLOB_APPEND)) {
        found->gl_pathc = 0;
        found->gl_pathv = NULL;
        if (!(flags & GLOB_DOOFFS))
            found->gl_offs = 0;
        size_t offs = found->gl_offs;
        found->gl_pathv = calloc(offs + 1, sizeof *found->gl_pathv);
        if (found->gl_pathv == NULL)
            return GLOB_NOSPACE;
    }
    size_t before = found->gl_pathc;
    int result = expand(*pattern == '/' ? "/" : "", pattern, flags, found);
    if (result != 0)
        return result;
    if (found->gl_pathc == before) {
        if (!(flags & GLOB_NOCHECK))
            return GLOB_NOMATCH;
        return add_path(found, pattern);
    }
    if (!(flags & GLOB_NOSORT))
        qsort(found->gl_pathv + found->gl_offs + before, found->gl_pathc - before, sizeof(char *), by_name);
    return 0;
}

void globfree(glob_t *found)
{
    if (found->gl_pathv != NULL) {
        for (size_t n = 0; n < found->gl_pathc; n++)
            free(found->gl_pathv[found->gl_offs + n]);
        free(found->gl_pathv);
    }
    found->gl_pathv = NULL;
    found->gl_pathc = 0;
}

/* ---- popen ----------------------------------------------------------- */

/* libc/src/spawn.rs: start the command with a pipe to it, and wait. */
int __taaj_popen(const char *command, int writing);
int __taaj_pclose(int fd);

FILE *popen(const char *command, const char *mode)
{
    if (command == NULL || mode == NULL || (mode[0] != 'r' && mode[0] != 'w')) {
        errno = EINVAL;
        return NULL;
    }
    int writing = mode[0] == 'w';
    int fd = __taaj_popen(command, writing);
    if (fd < 0)
        return NULL;
    FILE *stream = fdopen(fd, writing ? "w" : "r");
    if (stream == NULL) {
        int saved = errno;
        close(fd);
        __taaj_pclose(fd);
        errno = saved;
    }
    return stream;
}

int pclose(FILE *stream)
{
    if (stream == NULL) {
        errno = ECHILD;
        return -1;
    }
    /* Closed first: a command reading what this program wrote sees the
     * end, and can finish. */
    int fd = fileno(stream);
    fclose(stream);
    return __taaj_pclose(fd);
}

/* ---- execl, execlp, execle --------------------------------------------
 * The list forms of exec: the arguments one by one, ending in a null
 * pointer (execle's environment after it), gathered into an array for
 * the v forms (libc/src/spawn.rs).
 *
 * The va_list goes to the helpers as a pointer. Passed by value, it
 * would work on x86_64 only by accident: there va_list is an array, so
 * the helper's reads move the caller's list on too; on AArch64 it is a
 * struct, copied, and execle would then read its first argument again
 * where the environment should be. */

/* How many arguments up to and including the null: counted on a copy of
 * the list, since a va_list is read once. */
static size_t count_arguments(const char *first, va_list *list)
{
    va_list copy;
    va_copy(copy, *list);
    size_t n = 1;
    for (const char *a = first; a != NULL; a = va_arg(copy, const char *))
        n++;
    va_end(copy);
    return n;
}

/* The array: first, then the rest of *list up to its null (read), then
 * null. NULL with ENOMEM - the list read all the same. */
static char **gather(const char *first, va_list *list)
{
    char **argv = malloc(count_arguments(first, list) * sizeof(char *));
    size_t i = 0;
    for (const char *a = first; a != NULL; a = va_arg(*list, const char *))
        if (argv != NULL)
            argv[i++] = (char *)a;
    if (argv == NULL) {
        errno = ENOMEM;
        return NULL;
    }
    argv[i] = NULL;
    return argv;
}

int execl(const char *path, const char *argument, ...)
{
    va_list list;
    va_start(list, argument);
    char **argv = gather(argument, &list);
    va_end(list);
    if (argv == NULL)
        return -1;
    execv(path, argv);
    free(argv);
    return -1;
}

int execlp(const char *file, const char *argument, ...)
{
    va_list list;
    va_start(list, argument);
    char **argv = gather(argument, &list);
    va_end(list);
    if (argv == NULL)
        return -1;
    execvp(file, argv);
    free(argv);
    return -1;
}

int execle(const char *path, const char *argument, ...)
{
    va_list list;
    va_start(list, argument);
    char **argv = gather(argument, &list);
    /* gather read up to the null; the environment comes next. */
    char *const *environment = va_arg(list, char *const *);
    va_end(list);
    if (argv == NULL)
        return -1;
    execve(path, argv, environment);
    free(argv);
    return -1;
}
