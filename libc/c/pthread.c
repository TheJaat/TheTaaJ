/* pthread.c - POSIX threads for a program with one thread: what each
 * call means when no other thread can exist (pthread.h says why, and
 * what that comes to for each). */

#include <errno.h>
#include <pthread.h>
#include <stddef.h>
#include <stdlib.h>
#include <time.h>

/* ---- mutexes ---- */

int pthread_mutexattr_init(pthread_mutexattr_t *attr)
{
    attr->kind = PTHREAD_MUTEX_DEFAULT;
    return 0;
}

int pthread_mutexattr_destroy(pthread_mutexattr_t *attr)
{
    (void)attr;
    return 0;
}

int pthread_mutexattr_settype(pthread_mutexattr_t *attr, int kind)
{
    if (kind != PTHREAD_MUTEX_NORMAL && kind != PTHREAD_MUTEX_RECURSIVE && kind != PTHREAD_MUTEX_ERRORCHECK)
        return EINVAL;
    attr->kind = kind;
    return 0;
}

int pthread_mutexattr_gettype(const pthread_mutexattr_t *attr, int *kind)
{
    *kind = attr->kind;
    return 0;
}

int pthread_mutex_init(pthread_mutex_t *mutex, const pthread_mutexattr_t *attr)
{
    mutex->kind = attr ? attr->kind : PTHREAD_MUTEX_DEFAULT;
    mutex->depth = 0;
    return 0;
}

int pthread_mutex_destroy(pthread_mutex_t *mutex)
{
    return mutex->depth ? EBUSY : 0;
}

/* The one thread that could hold it is the one asking. So a mutex that
 * is held is held by the caller: a recursive one counts once more, and
 * any other kind could only wait forever - the deadlock POSIX speaks
 * of - which is reported instead. */
int pthread_mutex_lock(pthread_mutex_t *mutex)
{
    if (mutex->depth && mutex->kind != PTHREAD_MUTEX_RECURSIVE)
        return EDEADLK;
    mutex->depth++;
    return 0;
}

int pthread_mutex_trylock(pthread_mutex_t *mutex)
{
    if (mutex->depth && mutex->kind != PTHREAD_MUTEX_RECURSIVE)
        return EBUSY;
    mutex->depth++;
    return 0;
}

int pthread_mutex_unlock(pthread_mutex_t *mutex)
{
    if (mutex->depth == 0)
        return EPERM; /* not held by the caller - by anyone */
    mutex->depth--;
    return 0;
}

/* ---- conditions ---- */

int pthread_condattr_init(pthread_condattr_t *attr)
{
    (void)attr;
    return 0;
}

int pthread_condattr_destroy(pthread_condattr_t *attr)
{
    (void)attr;
    return 0;
}

int pthread_cond_init(pthread_cond_t *cond, const pthread_condattr_t *attr)
{
    (void)attr;
    cond->unused = 0;
    return 0;
}

int pthread_cond_destroy(pthread_cond_t *cond)
{
    (void)cond;
    return 0;
}

/* Nobody waits, so there is nobody to wake. */
int pthread_cond_signal(pthread_cond_t *cond)
{
    (void)cond;
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t *cond)
{
    (void)cond;
    return 0;
}

/* Only another thread could signal: waiting would never end. */
int pthread_cond_wait(pthread_cond_t *cond, pthread_mutex_t *mutex)
{
    (void)cond;
    if (mutex->depth == 0)
        return EPERM;
    return EDEADLK;
}

/* Waits out the time - nothing can end it sooner - with the mutex let
 * go meanwhile, as POSIX says, and taken again before returning. */
int pthread_cond_timedwait(pthread_cond_t *cond, pthread_mutex_t *mutex, const struct timespec *deadline)
{
    (void)cond;
    if (mutex->depth == 0)
        return EPERM;
    if (deadline->tv_nsec < 0 || deadline->tv_nsec >= 1000000000)
        return EINVAL;
    struct timespec now;
    clock_gettime(CLOCK_REALTIME, &now);
    if (now.tv_sec < deadline->tv_sec || (now.tv_sec == deadline->tv_sec && now.tv_nsec < deadline->tv_nsec)) {
        struct timespec left = {deadline->tv_sec - now.tv_sec, deadline->tv_nsec - now.tv_nsec};
        if (left.tv_nsec < 0) {
            left.tv_sec--;
            left.tv_nsec += 1000000000;
        }
        int depth = mutex->depth;
        mutex->depth = 0;
        nanosleep(&left, NULL);
        mutex->depth = depth;
    }
    return ETIMEDOUT;
}

/* ---- read-write locks: held for reading by any number, or for
 * writing by one ---- */

int pthread_rwlock_init(pthread_rwlock_t *lock, const pthread_rwlockattr_t *attr)
{
    (void)attr;
    lock->readers = 0;
    lock->writer = 0;
    return 0;
}

int pthread_rwlock_destroy(pthread_rwlock_t *lock)
{
    return lock->readers || lock->writer ? EBUSY : 0;
}

int pthread_rwlock_rdlock(pthread_rwlock_t *lock)
{
    if (lock->writer)
        return EDEADLK; /* the caller is the writer */
    lock->readers++;
    return 0;
}

int pthread_rwlock_tryrdlock(pthread_rwlock_t *lock)
{
    if (lock->writer)
        return EBUSY;
    lock->readers++;
    return 0;
}

int pthread_rwlock_wrlock(pthread_rwlock_t *lock)
{
    if (lock->writer || lock->readers)
        return EDEADLK;
    lock->writer = 1;
    return 0;
}

int pthread_rwlock_trywrlock(pthread_rwlock_t *lock)
{
    if (lock->writer || lock->readers)
        return EBUSY;
    lock->writer = 1;
    return 0;
}

int pthread_rwlock_unlock(pthread_rwlock_t *lock)
{
    if (lock->writer) {
        lock->writer = 0;
        return 0;
    }
    if (lock->readers) {
        lock->readers--;
        return 0;
    }
    return EPERM;
}

/* ---- once ---- */

int pthread_once(pthread_once_t *once, void (*function)(void))
{
    if (*once == 0) {
        *once = 1; /* first, so that the function may ask again and not recurse */
        function();
    }
    return 0;
}

/* ---- keys: one value each, for the one thread ---- */

static struct {
    int used;
    void (*destructor)(void *);
    const void *value;
} keys[PTHREAD_KEYS_MAX];

int pthread_key_create(pthread_key_t *key, void (*destructor)(void *))
{
    for (unsigned k = 0; k < PTHREAD_KEYS_MAX; k++) {
        if (!keys[k].used) {
            keys[k].used = 1;
            keys[k].destructor = destructor;
            keys[k].value = NULL;
            *key = k;
            return 0;
        }
    }
    return EAGAIN;
}

int pthread_key_delete(pthread_key_t key)
{
    if (key >= PTHREAD_KEYS_MAX || !keys[key].used)
        return EINVAL;
    keys[key].used = 0;
    return 0;
}

void *pthread_getspecific(pthread_key_t key)
{
    if (key >= PTHREAD_KEYS_MAX || !keys[key].used)
        return NULL;
    return (void *)keys[key].value;
}

int pthread_setspecific(pthread_key_t key, const void *value)
{
    if (key >= PTHREAD_KEYS_MAX || !keys[key].used)
        return EINVAL;
    keys[key].value = value;
    return 0;
}

/* ---- threads ---- */

int pthread_attr_init(pthread_attr_t *attr)
{
    attr->unused = 0;
    return 0;
}

int pthread_attr_destroy(pthread_attr_t *attr)
{
    (void)attr;
    return 0;
}

int pthread_attr_setdetachstate(pthread_attr_t *attr, int state)
{
    (void)attr;
    return state == PTHREAD_CREATE_JOINABLE || state == PTHREAD_CREATE_DETACHED ? 0 : EINVAL;
}

int pthread_attr_setstacksize(pthread_attr_t *attr, unsigned long size)
{
    (void)attr;
    (void)size;
    return 0;
}

pthread_t pthread_self(void)
{
    return 1;
}

int pthread_equal(pthread_t a, pthread_t b)
{
    return a == b;
}

/* No second thread can be made: POSIX's "the system lacked the
 * resources to create another thread". */
int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start)(void *), void *argument)
{
    (void)thread;
    (void)attr;
    (void)start;
    (void)argument;
    return EAGAIN;
}

/* There is no other thread to join or detach. */
int pthread_join(pthread_t thread, void **result)
{
    (void)result;
    return thread == 1 ? EDEADLK : ESRCH;
}

int pthread_detach(pthread_t thread)
{
    return thread == 1 ? 0 : ESRCH;
}

/* The last thread ending ends the process, with status 0 (POSIX). */
void pthread_exit(void *result)
{
    (void)result;
    exit(0);
}
