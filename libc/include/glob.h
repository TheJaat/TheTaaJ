/* glob.h - the paths that match a shell wildcard pattern ("src/*.c"),
 * found by reading the directories (fnmatch.h says what matches). */

#ifndef _GLOB_H
#define _GLOB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    size_t gl_pathc; /* how many matched */
    char **gl_pathv; /* them, after gl_offs nulls, then NULL */
    size_t gl_offs;  /* with GLOB_DOOFFS: how many nulls first */
} glob_t;

#define GLOB_ERR 0x01      /* stop at a directory that cannot be read */
#define GLOB_MARK 0x02     /* a '/' after each directory */
#define GLOB_NOSORT 0x04   /* in the order found, not by name */
#define GLOB_DOOFFS 0x08   /* gl_offs nulls first */
#define GLOB_NOCHECK 0x10  /* no match: the pattern itself */
#define GLOB_APPEND 0x20   /* add to what an earlier call found */
#define GLOB_NOESCAPE 0x40 /* a backslash is itself */

#define GLOB_NOSPACE 1
#define GLOB_ABORTED 2
#define GLOB_NOMATCH 3

/* 0, or one of the three above. errfunc is not called. */
int glob(const char *pattern, int flags, int (*errfunc)(const char *path, int error), glob_t *found);
void globfree(glob_t *found);

#ifdef __cplusplus
}
#endif

#endif
