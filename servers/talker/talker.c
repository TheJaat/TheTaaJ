/* talker - a ring 3 client.
 *
 * Looks up 'echo' by name and sends it a few messages. Two separate
 * processes, separate address spaces, communicating only through the
 * kernel - which is the property that makes a microkernel possible. */

#include <os/syscall.h>

static void Send(int Handle, const char *Text)
{
    SysPrint("[talker] sending '");
    SysPrint(Text);
    SysPrintLine("'");
    SysPipeWrite(Handle, Text, SysStringLength(Text));
    SysSleep(150);
}

int ModuleMain(void)
{
    int Echo;
    int Attempt;

    SysPrint("[talker] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    /* The server may not have registered yet - it is a separate process
     * and the scheduler decides the order. Retry rather than assume. */
    for (Attempt = 0; Attempt < 20; Attempt++) {
        Echo = SysLookupName("echo");
        if (Echo >= 0) {
            break;
        }
        SysSleep(50);
    }

    if (Echo < 0) {
        SysPrintLine("[talker] 'echo' is not registered, giving up");
        SysExit(1);
    }

    SysPrint("[talker] found 'echo' as handle ");
    SysPrintNumber((unsigned)Echo);
    SysPrint("\n");

    Send(Echo, "hello");
    Send(Echo, "from another process");
    Send(Echo, "quit");

    SysPrintLine("[talker] done");
    SysExit(0);
    return 0;
}
