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

/* Locale objects (POSIX 2008): a locale a program holds and hands to
 * the *_l functions, or makes its thread's own with uselocale, instead
 * of changing the whole program's with setlocale. There is one, "C",
 * so newlocale gives the same object for "C", "POSIX" and "" (the
 * user's locale, which is "C" too) and fails with ENOENT for any other
 * name; freelocale has nothing to free. libc++ is built on these
 * (ports/llvm/libcxx/include/__locale_dir/support/taaj.h). */
typedef struct __locale *locale_t;

#define LC_CTYPE_MASK    (1 << LC_CTYPE)
#define LC_NUMERIC_MASK  (1 << LC_NUMERIC)
#define LC_TIME_MASK     (1 << LC_TIME)
#define LC_COLLATE_MASK  (1 << LC_COLLATE)
#define LC_MONETARY_MASK (1 << LC_MONETARY)
#define LC_MESSAGES_MASK (1 << LC_MESSAGES)
#define LC_ALL_MASK      (LC_CTYPE_MASK | LC_NUMERIC_MASK | LC_TIME_MASK | \
                          LC_COLLATE_MASK | LC_MONETARY_MASK | LC_MESSAGES_MASK)

/* What uselocale returns for a thread using the program's locale, and
 * takes to make it do so again. */
#define LC_GLOBAL_LOCALE ((locale_t)-1)

/* newlocale(parts, name, base): `base` with the parts in `parts` (a
 * mask of LC_*_MASK) from the locale `name`; base 0 for a new one.
 * EINVAL for a mask with other bits or no name, ENOENT for a locale
 * there is not. On success `base` is not to be used again. */
locale_t newlocale(int parts, const char *name, locale_t base);
locale_t duplocale(locale_t locale);
void freelocale(locale_t locale);
/* uselocale(locale): makes `locale` the calling thread's (or, with
 * LC_GLOBAL_LOCALE, the program's again) and returns the one before;
 * with 0 only returns it. */
locale_t uselocale(locale_t locale);

#ifdef __cplusplus
}
#endif

#endif
