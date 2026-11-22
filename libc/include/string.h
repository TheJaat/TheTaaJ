/* string.h - strings, and plain memory.
 *
 * A string is bytes ending in a zero byte. Nothing records its length;
 * these functions walk to the zero. The mem functions are told a
 * length instead and do not care what the bytes are.
 *
 * Where one copies, the room at the destination is the caller's to
 * have got right. Nothing checks it. */
#ifndef _STRING_H
#define _STRING_H

#include <stddef.h>

/* Memory. memcpy's two blocks must not overlap; memmove's may. */
void *memcpy(void *to, const void *from, size_t count);
void *memmove(void *to, const void *from, size_t count);
void *memset(void *to, int byte, size_t count);
int memcmp(const void *a, const void *b, size_t count);
void *memchr(const void *in, int byte, size_t count);

/* Length: the bytes before the zero. strnlen looks at no more than max. */
size_t strlen(const char *s);
size_t strnlen(const char *s, size_t max);

/* Comparing: negative, zero or positive as a sorts before, the same
 * as, or after b. strncmp looks at no more than count bytes. */
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t count);

/* Copying. strncpy writes exactly count bytes, padding with zeros -
 * and writes no zero at all if from is count bytes or longer. */
char *strcpy(char *to, const char *from);
char *strncpy(char *to, const char *from, size_t count);

/* Adding from to the end of to. strncat adds at most count bytes, and
 * always a zero after them. */
char *strcat(char *to, const char *from);
char *strncat(char *to, const char *from, size_t count);

/* Finding: the address of what was found, or NULL. */
char *strchr(const char *s, int byte);    /* the first byte */
char *strrchr(const char *s, int byte);   /* the last */
char *strstr(const char *in, const char *find);

/* How many bytes at the start of s are all in set (strspn), or none of
 * them in set (strcspn). line[strcspn(line, "\n")] = 0 cuts a line at
 * its new line. */
size_t strspn(const char *s, const char *set);
size_t strcspn(const char *s, const char *set);

/* Where in s the first byte that is in set is, or NULL. */
char *strpbrk(const char *s, const char *set);

/* What an errno value means, in words. The string is the library's. */
char *strerror(int code);

/* A copy of s in memory from malloc, to be freed; NULL if none. */
char *strdup(const char *s);

#endif
