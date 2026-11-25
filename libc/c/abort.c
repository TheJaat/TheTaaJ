/* abort.c - abort(): the end of a program that has found itself in a
 * state it cannot go on from. It is the signal SIGABRT: a handler for
 * it runs first - and if the handler returns, the program ends all the
 * same, by the signal, which a shell reports as such. Nothing waiting
 * in a stream's buffer is written: whatever went wrong may have
 * spoiled it. */

#include <signal.h>
#include <stdlib.h>
#include <unistd.h>

_Noreturn void abort(void)
{
    raise(SIGABRT);
    /* Still here: caught, and the handler returned - or blocked. Not
     * again: the signal does what it does by default, now. */
    struct sigaction by_default = {0};
    sigaction(SIGABRT, &by_default, 0);
    sigset_t abort_only;
    sigemptyset(&abort_only);
    sigaddset(&abort_only, SIGABRT);
    sigprocmask(SIG_UNBLOCK, &abort_only, 0);
    raise(SIGABRT);
    _exit(128 + SIGABRT); /* not reached */
}
