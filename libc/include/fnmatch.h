/* fnmatch.h - whether a name matches a shell wildcard pattern: '*',
 * '?', and "[a-z]" (with '!' or '^' first for "not"); no character
 * classes ("[[:digit:]]"). 0 for a match, FNM_NOMATCH if not. */

#ifndef _FNMATCH_H
#define _FNMATCH_H

#ifdef __cplusplus
extern "C" {
#endif

#define FNM_NOMATCH 1
#define FNM_NOESCAPE 0x01 /* a backslash is itself, not an escape */
#define FNM_PATHNAME 0x02 /* a slash is matched only by a slash */
#define FNM_PERIOD 0x04   /* a leading period only by a period */

int fnmatch(const char *pattern, const char *string, int flags);

#ifdef __cplusplus
}
#endif

#endif
