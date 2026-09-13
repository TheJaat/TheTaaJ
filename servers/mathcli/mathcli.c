/* mathcli - a ring 3 client of the maths service.
 *
 * Knows nothing about the server except its name and the opcodes in the
 * shared interface header. No shared memory, no kernel calls into the
 * server - every interaction is a message. */

#include <os/rpc.h>
#include <os/mathsrv.h>

static int Call(int Service, unsigned Opcode, int A, int B)
{
    MathArgs_t Args;
    MathResult_t Result;
    int Length;

    Args.A = A;
    Args.B = B;
    Result.Value = 0;

    Length = RpcExecute(Service, Opcode, &Args, sizeof(Args),
                        &Result, sizeof(Result));
    if (Length < (int)sizeof(Result)) {
        SysPrintLine("[mathcli] call failed");
        return 0;
    }
    return Result.Value;
}

int ModuleMain(void)
{
    char Pong[8];
    int Service;
    int Attempt;

    SysPrint("[mathcli] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    if (RpcInitialize() != SYSCALL_OK) {
        SysPrintLine("[mathcli] no reply channel");
        SysExit(1);
    }

    /* The server is a separate process; the scheduler decides who runs
     * first. Retry rather than assume it has published yet. */
    for (Attempt = 0; Attempt < 20; Attempt++) {
        Service = RpcConnect(MATHSRV_NAME);
        if (Service >= 0) {
            break;
        }
        SysSleep(50);
    }
    if (Service < 0) {
        SysPrintLine("[mathcli] 'math' is not available");
        SysExit(1);
    }

    SysMemSet(Pong, 0, sizeof(Pong));
    if (RpcExecute(Service, MATH_PING, 0, 0, Pong, sizeof(Pong) - 1) > 0) {
        SysPrint("[mathcli] ping -> ");
        SysPrintLine(Pong);
    }

    SysPrint("[mathcli] 17 + 25 = ");
    SysPrintNumber((unsigned)Call(Service, MATH_ADD, 17, 25));
    SysPrint("\n");

    SysPrint("[mathcli] 6 * 7 = ");
    SysPrintNumber((unsigned)Call(Service, MATH_MUL, 6, 7));
    SysPrint("\n");

    {
        MathStats_t Stats;
        Stats.Requests = 0;
        Stats.Clients = 0;
        if (RpcExecute(Service, MATH_STATS, 0, 0, &Stats, sizeof(Stats))
            >= (int)sizeof(Stats)) {
            SysPrint("[mathcli] server has served ");
            SysPrintNumber(Stats.Requests);
            SysPrint(" requests from ");
            SysPrintNumber(Stats.Clients);
            SysPrintLine(" client(s)");
        }
    }

    SysPrintLine("[mathcli] done");
    SysExit(0);
    return 0;
}
