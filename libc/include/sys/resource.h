/* sys/resource.h - limits on what a program may use, and what it has
 * used.
 *
 * Two limits are real here: the number of descriptors (32,
 * RLIMIT_NOFILE), and memory (RLIMIT_AS and RLIMIT_DATA, both the
 * kernel's memory limit for the process: what it may take, counted as
 * pages are touched). Memory's can be lowered with setrlimit - and the
 * programs this one starts afterwards get it too - but never raised
 * (EPERM). Every other limit is "infinite" (RLIM_INFINITY), as Linux
 * says of a limit nobody set: the stack is the size the program was
 * linked to ask for. setrlimit changes nothing else - asking for what
 * is already so succeeds, anything else is EPERM. TheTaaJ does
 * not tell a program its own use of the processor yet: there is no
 * getrusage. The numbers are Linux's. */

#ifndef _SYS_RESOURCE_H
#define _SYS_RESOURCE_H

#include <sys/time.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned long rlim_t;

struct rlimit {
    rlim_t rlim_cur;
    rlim_t rlim_max;
};

#define RLIM_INFINITY (~0UL)

#define RLIMIT_CPU 0
#define RLIMIT_FSIZE 1
#define RLIMIT_DATA 2
#define RLIMIT_STACK 3
#define RLIMIT_CORE 4
#define RLIMIT_RSS 5
#define RLIMIT_NPROC 6
#define RLIMIT_NOFILE 7
#define RLIMIT_MEMLOCK 8
#define RLIMIT_AS 9

int getrlimit(int resource, struct rlimit *limit);
int setrlimit(int resource, const struct rlimit *limit);

/* What wait4 would say a child used; there are no children. */
struct rusage {
    struct timeval ru_utime;
    struct timeval ru_stime;
    long ru_maxrss;
    long ru_ixrss;
    long ru_idrss;
    long ru_isrss;
    long ru_minflt;
    long ru_majflt;
    long ru_nswap;
    long ru_inblock;
    long ru_oublock;
    long ru_msgsnd;
    long ru_msgrcv;
    long ru_nsignals;
    long ru_nvcsw;
    long ru_nivcsw;
};

#define RUSAGE_SELF 0
#define RUSAGE_CHILDREN (-1)

#ifdef __cplusplus
}
#endif

#endif
