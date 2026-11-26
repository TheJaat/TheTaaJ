/* locale.c - the one locale there is, "C" (locale.h), its locale
 * object, and the two string functions that depend on a locale: in
 * "C", comparing letters is comparing their codes, so strcoll is strcmp
 * and strxfrm a copy. */

#include <errno.h>
#include <langinfo.h>
#include <limits.h>
#include <locale.h>
#include <string.h>

/* The names of "C": "POSIX" too, and "", the user's locale. */
static int known(const char *name)
{
    return strcmp(name, "C") == 0 || strcmp(name, "POSIX") == 0 || name[0] == '\0';
}

char *setlocale(int part, const char *name)
{
    static char c[] = "C";
    if (part < LC_CTYPE || part > LC_ALL)
        return NULL;
    if (name == NULL || known(name))
        return c;
    return NULL;
}

/* The locale object of "C": every locale_t there is points here. */
struct __locale {
    char name[2];
};
static struct __locale c_locale = { "C" };

locale_t newlocale(int parts, const char *name, locale_t base)
{
    (void)base; /* nothing to free: it is c_locale too */
    if ((parts & ~LC_ALL_MASK) != 0 || name == NULL) {
        errno = EINVAL;
        return 0;
    }
    if (!known(name)) {
        errno = ENOENT;
        return 0;
    }
    return &c_locale;
}

locale_t duplocale(locale_t locale)
{
    (void)locale;
    return &c_locale;
}

void freelocale(locale_t locale)
{
    (void)locale;
}

/* Each thread's locale: LC_GLOBAL_LOCALE until it says otherwise. */
static _Thread_local locale_t thread_locale = LC_GLOBAL_LOCALE;

locale_t uselocale(locale_t locale)
{
    locale_t before = thread_locale;
    if (locale != 0)
        thread_locale = locale;
    return before;
}

struct lconv *localeconv(void)
{
    static char point[] = ".", none[] = "";
    static struct lconv c = {
        point, none, none, none, none, none, none, none, none, none,
        CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX,
        CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX, CHAR_MAX,
    };
    return &c;
}

int strcoll(const char *a, const char *b)
{
    return strcmp(a, b);
}

/* strxfrm(to, from, size): `from` made into a string that strcmp
 * orders as strcoll orders the original - in "C", itself. Its length
 * is returned; if that is size or more, `to` holds nothing useful. */
size_t strxfrm(char *to, const char *from, size_t size)
{
    size_t length = strlen(from);
    if (length < size)
        memcpy(to, from, length + 1);
    return length;
}

char *nl_langinfo(nl_item item)
{
    static char codeset[] = "ASCII", point[] = ".", date_time[] = "%a %b %e %H:%M:%S %Y",
                date[] = "%m/%d/%y", time[] = "%H:%M:%S", none[] = "";
    switch (item) {
    case CODESET: return codeset;
    case RADIXCHAR: return point;
    case D_T_FMT: return date_time;
    case D_FMT: return date;
    case T_FMT: return time;
    default: return none;
    }
}
