/* abort.c - abort(): the end of a program that has found itself in a
 * state it cannot go on from. On Unix it is the signal SIGABRT, and a
 * shell reports such a program as ended with code 128 + 6 = 134; there
 * are no signals here, so the code is given directly. Nothing waiting
 * in a stream's buffer is written: whatever went wrong may have
 * spoiled it. */

#include <stdlib.h>
#include <unistd.h>

_Noreturn void abort(void)
{
    _exit(134);
}
