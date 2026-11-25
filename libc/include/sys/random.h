/* sys/random.h - random bytes from the kernel (libc/src/random.rs).
 *
 * TheTaaJ's kernel seeds its generator before the first program
 * starts, so getrandom never blocks and never fails for want of
 * entropy; both flags are accepted and change nothing. */
#ifndef _SYS_RANDOM_H
#define _SYS_RANDOM_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GRND_NONBLOCK 1
#define GRND_RANDOM 2
#define GRND_INSECURE 4

ssize_t getrandom(void *buffer, size_t length, unsigned int flags);
int getentropy(void *buffer, size_t length);

#ifdef __cplusplus
}
#endif

#endif
