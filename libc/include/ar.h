/* ar.h - the layout of an archive (a library, libc.a): "!<arch>\n",
 * then for each member a header and its bytes, padded to an even
 * length. What make reads to find a member's time (lib.a(x.o)). */

#ifndef _AR_H
#define _AR_H

#define ARMAG "!<arch>\n"
#define SARMAG 8
#define ARFMAG "`\n"

struct ar_hdr {
    char ar_name[16]; /* the name, ending in '/' (GNU) or a space */
    char ar_date[12]; /* the time, decimal */
    char ar_uid[6];
    char ar_gid[6];
    char ar_mode[8];  /* octal */
    char ar_size[10]; /* the member's length, decimal */
    char ar_fmag[2];  /* ARFMAG */
};

#endif
