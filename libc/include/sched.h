/* sched.h - giving the processor up, and choosing the CPUs and the
 * policy a program runs with (libc's sched.rs; journal entry 124).
 *
 * The policy is always SCHED_OTHER, its priority 0: how important a
 * program is, is its nice value (setpriority, nice). SCHED_FIFO and
 * SCHED_RR are EPERM until the kernel has real-time classes. A cpu_set_t
 * holds CPU_SETSIZE (64) CPUs. */

#ifndef _SCHED_H
#define _SCHED_H

#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Lets the kernel run something else now; this program carries on at
 * its next turn. Always 0. */
int sched_yield(void);

#define SCHED_OTHER 0
#define SCHED_FIFO 1
#define SCHED_RR 2

struct sched_param {
    int sched_priority;
};

int sched_setscheduler(pid_t pid, int policy, const struct sched_param *param);
int sched_getscheduler(pid_t pid);
int sched_setparam(pid_t pid, const struct sched_param *param);
int sched_getparam(pid_t pid, struct sched_param *param);
int sched_get_priority_max(int policy);
int sched_get_priority_min(int policy);

#define CPU_SETSIZE 64
typedef struct {
    unsigned long __bits[CPU_SETSIZE / (8 * sizeof(unsigned long))];
} cpu_set_t;

#define CPU_ZERO(set) ((set)->__bits[0] = 0)
#define CPU_SET(cpu, set) ((cpu) < CPU_SETSIZE ? ((set)->__bits[0] |= 1UL << (cpu)) : 0)
#define CPU_CLR(cpu, set) ((cpu) < CPU_SETSIZE ? ((set)->__bits[0] &= ~(1UL << (cpu))) : 0)
#define CPU_ISSET(cpu, set) ((cpu) < CPU_SETSIZE && ((set)->__bits[0] >> (cpu)) & 1UL)
#define CPU_COUNT(set) __builtin_popcountl((set)->__bits[0])

/* The CPUs a thread may run on: pid 0 is the calling thread, a child's
 * pid every thread of the child. */
int sched_setaffinity(pid_t pid, size_t size, const cpu_set_t *set);
int sched_getaffinity(pid_t pid, size_t size, cpu_set_t *set);

#ifdef __cplusplus
}
#endif

#endif
