/* echo - a ring 3 server.
 *
 * Creates a pipe, publishes it under a name, and echoes back whatever is
 * written to it. It never calls a kernel function: everything goes
 * through int 0x80.
 *
 * This is the shape every driver will take once interrupts and io-spaces
 * can be delegated - a loop around a blocking read on a pipe. */

#include <os/syscall.h>

#define REQUEST_MAX     64

int ModuleMain(void)
{
    char Buffer[REQUEST_MAX];
    int Pipe;
    int Count = 0;

    SysPrint("[echo] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Pipe = SysPipeCreate(256, 0);
    if (Pipe < 0) {
        SysPrintLine("[echo] could not create a pipe");
        SysExit(1);
    }

    if (SysRegisterName("echo", Pipe) != SYSCALL_OK) {
        SysPrintLine("[echo] could not register the name");
        SysExit(1);
    }
    SysPrintLine("[echo] registered as 'echo', waiting");

    for (;;) {
        /* Blocks. While nothing is being sent this server costs exactly
         * nothing - it is not scheduled at all. */
        int Length = SysPipeRead(Pipe, Buffer, REQUEST_MAX - 1);

        if (Length <= 0) {
            SysPrintLine("[echo] read failed, exiting");
            break;
        }

        Buffer[Length] = '\0';
        Count++;

        SysPrint("[echo] received '");
        SysPrint(Buffer);
        SysPrint("' (");
        SysPrintNumber((unsigned)Count);
        SysPrintLine(" so far)");

        if (SysStringCompare(Buffer, "quit") == 0) {
            SysPrintLine("[echo] asked to quit");
            break;
        }
    }

    SysPrint("[echo] handled ");
    SysPrintNumber((unsigned)Count);
    SysPrintLine(" messages");
    SysExit(0);
    return 0;
}
