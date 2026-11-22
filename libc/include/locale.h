/* locale.h - how numbers, money, dates and letters are written in a
 * language and a country. TheTaaJ has one locale, the one C starts in,
 * "C" (also called "POSIX"): a point before the decimals, no grouping
 * of thousands, English month names, letters compared by their codes.
 * libc/c/locale.c. */
#ifndef _LOCALE_H
#define _LOCALE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The parts of a locale, which setlocale can change one at a time. */
#define LC_CTYPE    0 /* what is a letter, a digit; multibyte text */
#define LC_NUMERIC  1 /* the decimal point */
#define LC_TIME     2 /* strftime's names */
#define LC_COLLATE  3 /* strcoll's order */
#define LC_MONETARY 4 /* localeconv's money */
#define LC_MESSAGES 5 /* the language of messages */
#define LC_ALL      6 /* all of them */

/* setlocale(part, name): makes `name` the locale for `part`, and says
 * which locale that is now - or, with name NULL, only says. "C",
 * "POSIX" and "" (the user's choice: there is none) are all "C"; any
 * other name is refused with NULL, and nothing changes. */
char *setlocale(int part, const char *name);

/* What localeconv describes: how this locale writes numbers and money.
 * A char of CHAR_MAX means "not given". */
struct lconv {
    char *decimal_point;     /* "." */
    char *thousands_sep;     /* "" */
    char *grouping;          /* "" */
    char *int_curr_symbol;   /* "" */
    char *currency_symbol;   /* "" */
    char *mon_decimal_point; /* "" */
    char *mon_thousands_sep; /* "" */
    char *mon_grouping;      /* "" */
    char *positive_sign;     /* "" */
    char *negative_sign;     /* "" */
    char int_frac_digits;    /* the rest CHAR_MAX */
    char frac_digits;
    char p_cs_precedes;
    char p_sep_by_space;
    char n_cs_precedes;
    char n_sep_by_space;
    char p_sign_posn;
    char n_sign_posn;
    char int_p_cs_precedes;
    char int_p_sep_by_space;
    char int_n_cs_precedes;
    char int_n_sep_by_space;
    char int_p_sign_posn;
    char int_n_sign_posn;
};

struct lconv *localeconv(void);

#ifdef __cplusplus
}
#endif

#endif
