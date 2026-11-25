/* pthread.h - POSIX threads (libc/src/threads.rs).
 *
 * A program on TheTaaJ may have many threads, on every CPU at once.
 * The kernel gives each its own stack and thread pointer, and a futex
 * to sleep on; the rest is in the program:
 *
 *   threads      pthread_create, join, detach, exit, self. A thread's
 *                stack is 8 MiB unless its attributes say otherwise -
 *                reserved: only the pages it touches take memory
 *   mutexes      normal, recursive and error-checking. One nobody else
 *                wants costs one atomic instruction to take and one to
 *                give back; a thread that has to wait sleeps in the
 *                kernel. Locking a normal mutex again from the thread
 *                that holds it says EDEADLK instead of waiting for ever
 *   conditions   wait, timedwait (CLOCK_REALTIME, as POSIX says),
 *                signal, broadcast
 *   rwlocks      many readers or one writer
 *   once, keys   pthread_once; keys with destructors, run at each
 *                thread's end
 *
 * errno is per thread, and so are C++'s thread_local variables.
 *
 * The types keep the sizes they had when a program had one thread, so
 * code compiled against them then (libc++) still fits. */

#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <sched.h>
#include <time.h>

/* C11's _Noreturn is C++'s [[noreturn]]. */
#if defined(__cplusplus) && !defined(_Noreturn)
#define _Noreturn [[noreturn]]
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned long pthread_t;

typedef struct {
    int kind;   /* PTHREAD_MUTEX_NORMAL, _RECURSIVE or _ERRORCHECK, and
                 * above the low 8 bits how many more times its owner
                 * holds a recursive one */
    int state;  /* 0 free; else the owner's id, and whether anyone waits */
} pthread_mutex_t;

typedef struct {
    int kind;
} pthread_mutexattr_t;

typedef struct {
    int sequence; /* moved on by every signal */
} pthread_cond_t;

typedef struct {
    int unused;
} pthread_condattr_t;

typedef struct {
    int detached;
    int unused;
    unsigned long stack_size;    /* 0: the default, 8 MiB */
    void *stack_address;         /* the program's own stack, or 0 */
    unsigned long guard_size;    /* said back; the guard is one page */
    long spare[3];               /* 56 bytes in all, as glibc's */
} pthread_attr_t;

typedef struct {
    int state;    /* how many read; all bits set: one writes */
    int sequence; /* moved on by every unlock */
} pthread_rwlock_t;

typedef struct {
    int unused;
} pthread_rwlockattr_t;

typedef int pthread_once_t;
typedef unsigned pthread_key_t;

#define PTHREAD_MUTEX_NORMAL 0
#define PTHREAD_MUTEX_RECURSIVE 1
#define PTHREAD_MUTEX_ERRORCHECK 2
#define PTHREAD_MUTEX_DEFAULT PTHREAD_MUTEX_NORMAL

#define PTHREAD_MUTEX_INITIALIZER {PTHREAD_MUTEX_NORMAL, 0}
#define PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP {PTHREAD_MUTEX_RECURSIVE, 0}
#define PTHREAD_COND_INITIALIZER {0}
#define PTHREAD_RWLOCK_INITIALIZER {0, 0}
#define PTHREAD_ONCE_INIT 0

#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1

/* How many keys a program may make, and how many times the
 * destructors are run at a thread's end while values are left. */
#define PTHREAD_KEYS_MAX 128
#define PTHREAD_DESTRUCTOR_ITERATIONS 4

/* The smallest stack a thread may be given. */
#define PTHREAD_STACK_MIN 16384

int pthread_mutexattr_init(pthread_mutexattr_t *attr);
int pthread_mutexattr_destroy(pthread_mutexattr_t *attr);
int pthread_mutexattr_settype(pthread_mutexattr_t *attr, int kind);
int pthread_mutexattr_gettype(const pthread_mutexattr_t *attr, int *kind);

int pthread_mutex_init(pthread_mutex_t *mutex, const pthread_mutexattr_t *attr);
int pthread_mutex_destroy(pthread_mutex_t *mutex);
int pthread_mutex_lock(pthread_mutex_t *mutex);
int pthread_mutex_trylock(pthread_mutex_t *mutex);
int pthread_mutex_unlock(pthread_mutex_t *mutex);

int pthread_condattr_init(pthread_condattr_t *attr);
int pthread_condattr_destroy(pthread_condattr_t *attr);
int pthread_cond_init(pthread_cond_t *cond, const pthread_condattr_t *attr);
int pthread_cond_destroy(pthread_cond_t *cond);
int pthread_cond_signal(pthread_cond_t *cond);
int pthread_cond_broadcast(pthread_cond_t *cond);
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex);
int pthread_cond_timedwait(pthread_cond_t *cond, pthread_mutex_t *mutex, const struct timespec *deadline);

int pthread_rwlock_init(pthread_rwlock_t *lock, const pthread_rwlockattr_t *attr);
int pthread_rwlock_destroy(pthread_rwlock_t *lock);
int pthread_rwlock_rdlock(pthread_rwlock_t *lock);
int pthread_rwlock_tryrdlock(pthread_rwlock_t *lock);
int pthread_rwlock_wrlock(pthread_rwlock_t *lock);
int pthread_rwlock_trywrlock(pthread_rwlock_t *lock);
int pthread_rwlock_unlock(pthread_rwlock_t *lock);

int pthread_once(pthread_once_t *once, void (*function)(void));

int pthread_key_create(pthread_key_t *key, void (*destructor)(void *));
int pthread_key_delete(pthread_key_t key);
void *pthread_getspecific(pthread_key_t key);
int pthread_setspecific(pthread_key_t key, const void *value);

int pthread_attr_init(pthread_attr_t *attr);
int pthread_attr_destroy(pthread_attr_t *attr);
int pthread_attr_setdetachstate(pthread_attr_t *attr, int state);
int pthread_attr_getdetachstate(const pthread_attr_t *attr, int *state);
int pthread_attr_setstacksize(pthread_attr_t *attr, unsigned long size);
int pthread_attr_getstacksize(const pthread_attr_t *attr, unsigned long *size);
int pthread_attr_setstack(pthread_attr_t *attr, void *address, unsigned long size);
int pthread_attr_getstack(const pthread_attr_t *attr, void **address, unsigned long *size);
int pthread_attr_setguardsize(pthread_attr_t *attr, unsigned long size);
int pthread_attr_getguardsize(const pthread_attr_t *attr, unsigned long *size);
/* thread's attributes now: detached or not, and where its stack is
 * (the main thread's too). Not POSIX: glibc's (_np), which Rust's std
 * and others use. */
int pthread_getattr_np(pthread_t thread, pthread_attr_t *attr);

pthread_t pthread_self(void);
int pthread_equal(pthread_t a, pthread_t b);
int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start)(void *), void *argument);
int pthread_join(pthread_t thread, void **result);
int pthread_detach(pthread_t thread);
_Noreturn void pthread_exit(void *result);

#ifdef __cplusplus
}
#endif

#endif
