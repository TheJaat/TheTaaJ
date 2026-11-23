/* libgen.h - the parts of a path. Both may change the path in place. */

#ifndef _LIBGEN_H
#define _LIBGEN_H

#ifdef __cplusplus
extern "C" {
#endif

/* The last name: "a/b/" is "b", "/" is "/", "" or NULL is ".". */
char *basename(char *path);
/* All but the last name: "a/b" is "a", "/a" is "/", "a" is ".". */
char *dirname(char *path);

#ifdef __cplusplus
}
#endif

#endif
