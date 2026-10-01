/* calccli - a client over synchronous IPC and shared memory. */

#include <os/syscall.h>
#include <os/calcsrv.h>

int ModuleMain(void)
{
    int Endpoint = -1, Shm = -1;
    int Attempt;

    SysPrint("[calccli] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    for (Attempt = 0; Attempt < 20; Attempt++) {
        if (SysLookup(CALCSRV_NAME, &Endpoint, &Shm) == 0) {
            break;
        }
        SysSleep(50);
    }
    if (Endpoint < 0) {
        SysPrintLine("[calccli] 'calc' is not available");
        SysExit(1);
    }

    SysPrint("[calccli] got endpoint handle ");
    SysPrintNumber((unsigned)Endpoint);
    SysPrint(", shm handle ");
    SysPrintNumber((unsigned)Shm);
    SysPrint("\n");

    /* ping */
    {
        char Pong[8];
        SysMemSet(Pong, 0, sizeof(Pong));
        if (SysCall2(Endpoint, CALC_PING, 0, 0, Pong, sizeof(Pong) - 1) > 0) {
            SysPrint("[calccli] ping -> ");
            SysPrintLine(Pong);
        }
    }

    /* an ordinary call */
    {
        CalcArgs_t a; CalcResult_t r;
        a.A = 111; a.B = 222; r.Value = 0;
        SysCall2(Endpoint, CALC_ADD, &a, sizeof(a), &r, sizeof(r));
        SysPrint("[calccli] 111 + 222 = ");
        SysPrintNumber((unsigned)r.Value);
        SysPrint("\n");
    }

    /* the badge: the server is told who we are, we never said */
    {
        CalcWho_t w; w.Badge = 0;
        SysCall2(Endpoint, CALC_WHOAMI, 0, 0, &w, sizeof(w));
        SysPrint("[calccli] server sees my badge as ");
        SysPrintNumber(w.Badge);
        SysPrint(" (my pid is ");
        SysPrintNumber((unsigned)SysGetPid());
        SysPrintLine(")");
    }

    /* bulk through shared memory: 256 integers, none of which cross a
       message boundary */
    if (Shm >= 0) {
        unsigned Base = SysShmMap(Shm);

        if (Base == 0) {
            SysPrintLine("[calccli] could not map the shared region");
        }
        else {
            int *Data = (int*)Base;
            CalcShm_t q; CalcResult_t r;
            unsigned i;
            int Expected = 0;

            SysPrint("[calccli] shared region mapped at 0x");
            SysPrintNumber(Base);
            SysPrint("\n");

            for (i = 0; i < 256; i++) {
                Data[i] = (int)(i + 1);
                Expected += (int)(i + 1);
            }

            q.Count = 256;
            r.Value = 0;
            SysCall2(Endpoint, CALC_SUM_SHM, &q, sizeof(q), &r, sizeof(r));

            SysPrint("[calccli] server summed them to ");
            SysPrintNumber((unsigned)r.Value);
            SysPrint(", expected ");
            SysPrintNumber((unsigned)Expected);
            SysPrintLine(r.Value == Expected ? "  MATCH" : "  MISMATCH");
            SysPrintLine("[calccli] 1 KB of data, 4 bytes of message");
        }
    }

    SysPrintLine("[calccli] done");
    SysExit(0);
    return 0;
}
