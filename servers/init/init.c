/* init - the supervisor.
 *
 * Starts the drivers and restarts any that die. This is the payoff a
 * microkernel is actually for: a driver fault kills one process, and
 * something else notices and brings it back. In a monolithic kernel the
 * equivalent event is a reboot.
 *
 * It needs no kernel support beyond spawn and a liveness check, because
 * a dying process already releases its interrupt line, its ports and its
 * handles - the kernel cleans up, so a restart finds the hardware free. */

#include <os/syscall.h>

#define SUPERVISED_MAX  4
#define POLL_MS         500

typedef struct _Service {
    const char *Module;
    int         Pid;
    unsigned    Restarts;
    int         Failed;
} Service_t;

static Service_t Services[SUPERVISED_MAX];
static int Count = 0;

static void Supervise(const char *Module)
{
    if (Count < SUPERVISED_MAX) {
        Services[Count].Module   = Module;
        Services[Count].Pid      = -1;
        Services[Count].Restarts = 0;
        Services[Count].Failed   = 0;
        Count++;
    }
}

static void Start(Service_t *Service)
{
    int Pid = SysSpawn(Service->Module);

    if (Pid < 0) {
        SysPrint("[init] could not start ");
        SysPrintLine(Service->Module);
        Service->Failed = 1;
        return;
    }

    Service->Pid = Pid;
    SysPrint("[init] started ");
    SysPrint(Service->Module);
    SysPrint(" as pid ");
    SysPrintNumber((unsigned)Pid);
    SysPrint("\n");
}

int ModuleMain(void)
{
    int i;

    SysPrint("[init] supervisor starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Supervise("pci.mod");
    Supervise("serial.mod");

    for (i = 0; i < Count; i++) {
        Start(&Services[i]);
        /* Stagger the starts. Two servers registering names at once is
         * fine, but a failure is far easier to read when the output of
         * one has finished before the next begins. */
        SysSleep(200);
    }

    SysPrintLine("[init] supervising");

    for (;;) {
        SysSleep(POLL_MS);

        for (i = 0; i < Count; i++) {
            Service_t *Service = &Services[i];

            if (Service->Failed || Service->Pid < 0) {
                continue;
            }
            if (SysProcessAlive(Service->Pid)) {
                continue;
            }

            SysPrint("[init] ");
            SysPrint(Service->Module);
            SysPrint(" (pid ");
            SysPrintNumber((unsigned)Service->Pid);
            SysPrintLine(") died - restarting");

            Service->Restarts++;
            if (Service->Restarts > 5) {
                /* Something is wrong that restarting will not fix.
                 * Restarting forever would turn a broken driver into a
                 * livelock that also drowns the log. */
                SysPrint("[init] ");
                SysPrint(Service->Module);
                SysPrintLine(" keeps dying - giving up on it");
                Service->Failed = 1;
                continue;
            }

            Start(Service);
        }
    }

    return 0;
}
