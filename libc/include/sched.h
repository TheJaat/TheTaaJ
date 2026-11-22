/* sched.h - giving the processor up. */

#ifndef _SCHED_H
#define _SCHED_H

#ifdef __cplusplus
extern "C" {
#endif

/* Lets the kernel run something else now; this program carries on at
 * its next turn. Always 0. */
int sched_yield(void);

#ifdef __cplusplus
}
#endif

#endif
