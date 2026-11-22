/* locale.c - the one locale there is, "C" (locale.h), and the two
 * string functions that depend on a locale: in "C", comparing letters
 * is comparing their codes, so strcoll is strcmp and strxfrm a copy. */

#include <langinfo.h>
#include <limits.h>
#include <locale.h>
#include <string.h>

char *setlocale(int part, const char *name)
{
    static char c[] = "C";
    if (part < LC_CTYPE || part > LC_ALL)
        return NULL;
    if (name == NULL || strcmp(name, "C") == 0 || strcmp(name, "POSIX") == 0 || name[0] == '\0')
        return c;
    return NULL;
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
