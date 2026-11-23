/* ctype.c - what kind of character a byte is.
 *
 * Many C libraries answer these from a table of 257 entries, one per
 * possible argument, with a bit for each question. ASCII was laid out
 * so that no table is needed: the digits are in a row, each alphabet
 * is in a row, and everything printed lies between the space and the
 * tilde. So each question here is a comparison or two.
 *
 * Any argument that is not one of ASCII's 128 - the bytes from 128 up,
 * and EOF - falls outside every range, and is nothing. */

#include <ctype.h>
#include <strings.h>

int isdigit(int c)
{
    return c >= '0' && c <= '9';
}

int isupper(int c)
{
    return c >= 'A' && c <= 'Z';
}

int islower(int c)
{
    return c >= 'a' && c <= 'z';
}

int isalpha(int c)
{
    return isupper(c) || islower(c);
}

int isalnum(int c)
{
    return isalpha(c) || isdigit(c);
}

int isxdigit(int c)
{
    return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
}

/* The tab (9), new line, vertical tab, form feed and carriage return
 * (13) are five in a row. */
int isspace(int c)
{
    return c == ' ' || (c >= '\t' && c <= '\r');
}

int isblank(int c)
{
    return c == ' ' || c == '\t';
}

int isprint(int c)
{
    return c >= ' ' && c <= '~';
}

int isgraph(int c)
{
    return c > ' ' && c <= '~';
}

int ispunct(int c)
{
    return isgraph(c) && !isalnum(c);
}

int iscntrl(int c)
{
    return (c >= 0 && c < ' ') || c == 127;
}

/* A capital and its small letter are 32 apart: one bit. */
int toupper(int c)
{
    return islower(c) ? c - ('a' - 'A') : c;
}

int tolower(int c)
{
    return isupper(c) ? c + ('a' - 'A') : c;
}

int isascii(int c)
{
    return c >= 0 && c < 128;
}

int toascii(int c)
{
    return c & 0x7f;
}

/* strings.h: comparing as if both were lower case. */
int strcasecmp(const char *a, const char *b)
{
    for (;; a++, b++) {
        int x = tolower((unsigned char)*a), y = tolower((unsigned char)*b);
        if (x != y || x == 0)
            return x - y;
    }
}

int strncasecmp(const char *a, const char *b, size_t n)
{
    for (; n > 0; n--, a++, b++) {
        int x = tolower((unsigned char)*a), y = tolower((unsigned char)*b);
        if (x != y || x == 0)
            return x - y;
    }
    return 0;
}
