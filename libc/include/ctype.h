/* ctype.h - what kind of character a byte is.
 *
 * Each takes a byte as a number from 0 to 255, or EOF - which is what
 * getc returns, so its answer can be passed straight in. A plain char
 * that may be negative must be cast first:
 *
 *     isalpha((unsigned char)text[n])
 *
 * The characters are ASCII's. Bytes from 128 up are none of these
 * things: text here is UTF-8, where they are pieces of a longer
 * character, and one piece alone is not a letter. */
#ifndef _CTYPE_H
#define _CTYPE_H

int isalpha(int c);  /* a letter */
int isdigit(int c);  /* 0 to 9 */
int isalnum(int c);  /* a letter or a digit */
int isupper(int c);  /* A to Z */
int islower(int c);  /* a to z */
int isxdigit(int c); /* a digit of a hexadecimal number */
int isspace(int c);  /* a space, a tab, a new line, and the three like it */
int isblank(int c);  /* a space or a tab */
int isprint(int c);  /* takes room on the screen, the space included */
int isgraph(int c);  /* the same, without the space */
int ispunct(int c);  /* printed, and not a letter, a digit or a space */
int iscntrl(int c);  /* not printed: below the space, and 127 */

/* The other case of a letter; anything else as it was. */
int toupper(int c);
int tolower(int c);

#endif
