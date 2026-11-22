/* pthread.h - POSIX threads, for a program with one thread.
 *
 * A program on TheTaaJ has one thread today: the kernel has threads,
 * but no way yet for a program to start a second one of its own. So
 * this is POSIX's interface with that one thread behind it - not
 * stubs that pretend, but what each call really means when there is no
 * one else:
 *
 *   mutexes      real: a held mutex is held. Locking it again is the
 *                deadlock POSIX describes, reported as EDEADLK rather
 *                than waited out forever; a recursive one counts.
 *   conditions   signal and broadcast wake nobody, which is right;
 *                waiting could only end by another thread's signal, so
 *                pthread_cond_wait fails with EDEADLK at once, and
 *                pthread_cond_timedwait sleeps to its deadline and
 *                says ETIMEDOUT
 *   once         runs the function the first time
 *   keys         a value per key, for the one thread
 *   threads      pthread_self is 1; pthread_create fails with EAGAIN
 *                (no more threads may be made), so std::thread throws
 *
 * C++'s <mutex>, <condition_variable> and <thread> are built on these
 * (libc++ uses its pthread backend), and so is LLVM, which needs a
 * mutex even when it runs on one thread. */

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
    int kind;   /* PTHREAD_MUTEX_NORMAL, _RECURSIVE or _ERRORCHECK */
    int depth;  /* how many times it is held: 0 is free */
} pthread_mutex_t;

typedef struct {
    int kind;
} pthread_mutexattr_t;

typedef struct {
    int unused;
} pthread_cond_t;

typedef struct {
    int unused;
} pthread_condattr_t;

typedef struct {
    int unused;
} pthread_attr_t;

typedef struct {
    int readers;
    int writer;
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

/* How many keys a program may make. */
#define PTHREAD_KEYS_MAX 128

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
int pthread_attr_setstacksize(pthread_attr_t *attr, unsigned long size);

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
