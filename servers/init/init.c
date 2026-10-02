/* init - the supervisor, and only that.
 *
 * It used to be the registry as well. Splitting them matters because
 * every process depends on the registry to find anything: if the
 * supervisor crashes, naming should survive, and if the registry
 * crashes, something should notice. One process cannot do both.
 *
 * init starts regsrv first and supervises it like any other child, so a
 * registry that dies is restarted - and it is the only child whose death
 * costs the running system anything, since existing capabilities keep
 * working while new lookups fail. */

#include <os/syscall.h>
#include <os/registry.h>

#define SUPERVISED_MAX  8
#define POLL_MS         500
#define RESTART_MAX     5

typedef struct _Child {
    const char *Module;
    int         Pid;
    unsigned    Restarts;
    int         Failed;
    int         Critical;       /* started before anything else */
} Child_t;

static Child_t Children[SUPERVISED_MAX];
static int ChildCount = 0;

static void Supervise(const char *Module, int Critical)
{
    if (ChildCount < SUPERVISED_MAX) {
        Children[ChildCount].Module   = Module;
        Children[ChildCount].Pid      = -1;
        Children[ChildCount].Restarts = 0;
        Children[ChildCount].Failed   = 0;
        Children[ChildCount].Critical = Critical;
        ChildCount++;
    }
}

static int Start(Child_t *Child)
{
    int Pid = SysSpawn(Child->Module);

    if (Pid < 0) {
        SysPrint("[init] could not start ");
        SysPrintLine(Child->Module);
        Child->Failed = 1;
        return 0;
    }
    Child->Pid = Pid;
    SysPrint("[init] started ");
    SysPrint(Child->Module);
    SysPrint(" as pid ");
    SysPrintNumber((unsigned)Pid);
    SysPrint("\n");
    return 1;
}

int ModuleMain(void)
{
    int i;

    SysPrint("[init] supervisor starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    /* The registry first and alone: every later server needs it to
     * publish, so starting anything alongside it is a race. */
    Supervise("regsrv.mod", 1);
    Supervise("pci.mod", 0);
    Supervise("serial.mod", 0);
    Supervise("ata.mod", 0);
    Supervise("fat32.mod", 0);

    for (i = 0; i < ChildCount; i++) {
        if (!Start(&Children[i])) {
            continue;
        }
        /* Give the registry longer: nothing else can publish until it
         * has nominated itself. */
        SysSleep(Children[i].Critical ? 400 : 200);
    }

    SysPrintLine("[init] supervising");

    for (;;) {
        SysSleep(POLL_MS);

        for (i = 0; i < ChildCount; i++) {
            Child_t *Child = &Children[i];

            if (Child->Failed || Child->Pid < 0) {
                continue;
            }
            if (SysProcessAlive(Child->Pid)) {
                continue;
            }

            SysPrint("[init] ");
            SysPrint(Child->Module);
            SysPrintLine(" died - restarting");

            Child->Restarts++;
            if (Child->Restarts > RESTART_MAX) {
                /* Restarting forever turns a broken driver into a
                 * livelock that also drowns the log. */
                SysPrint("[init] giving up on ");
                SysPrintLine(Child->Module);
                Child->Failed = 1;
                continue;
            }
            Start(Child);

            /* A registry restart loses every published name, so the
             * servers that depend on it must come back too. Their own
             * death will be noticed on a later pass; marking them now
             * would race with processes that are still exiting. */
            if (Child->Critical) {
                SysPrintLine("[init] the registry restarted - "
                             "published names were lost");
            }
        }
    }

    return 0;
}
