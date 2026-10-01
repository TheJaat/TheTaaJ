/* init - the registry and the supervisor.
 *
 * It is the first process the kernel starts, and it holds the one
 * capability the kernel hands out: its own endpoint, nominated with
 * SysSetRegistry so that every later process inherits a capability to
 * it at HANDLE_REGISTRY.
 *
 * From there, naming is entirely a user-space concern. The kernel has
 * no name table, no uniqueness rule and no opinion about who may claim
 * what - all of that is the code below. */

#include <os/syscall.h>
#include <os/registry.h>

#define SERVICE_MAX     8
#define SUPERVISED_MAX  4
#define POLL_MS         500

typedef struct _Service {
    char     Name[REGISTRY_NAME_MAX];
    int      Endpoint;          /* handle in THIS process */
    int      Shm;
    unsigned Owner;             /* badge of whoever published it */
    int      Used;
} Service_t;

static Service_t Services[SERVICE_MAX];

typedef struct _Child {
    const char *Module;
    int         Pid;
    unsigned    Restarts;
    int         Failed;
} Child_t;

static Child_t Children[SUPERVISED_MAX];
static int ChildCount = 0;
static int Endpoint = -1;

static int NameMatches(const char *A, const char *B)
{
    unsigned i;
    for (i = 0; i < REGISTRY_NAME_MAX; i++) {
        if (A[i] != B[i]) { return 0; }
        if (A[i] == '\0') { return 1; }
    }
    return 1;
}

/* HandlePublish
 * The publisher has already granted us its endpoint, so the message
 * only carries the handle index we now hold. We keep it; we do not
 * validate that it points anywhere useful, because we cannot - and do
 * not need to. A bad capability fails for the client that uses it, not
 * for us. */
static void HandlePublish(RegistryPublish_t *Request, unsigned Badge)
{
    RegistryReply_t Reply;
    int i, Slot = -1;

    Reply.Status = -1;
    Reply.Endpoint = -1;
    Reply.Shm = -1;

    for (i = 0; i < SERVICE_MAX; i++) {
        if (Services[i].Used && NameMatches(Services[i].Name, Request->Name)) {
            /* Taken. Only the original publisher may replace it - which
             * is a policy decision, and exactly the sort that belongs
             * here rather than in the kernel. */
            if (Services[i].Owner != Badge) {
                SysPrint("[init] '");
                SysPrint(Request->Name);
                SysPrintLine("' is already taken");
                SysReply(&Reply, sizeof(Reply));
                return;
            }
            Slot = i;
            break;
        }
        if (!Services[i].Used && Slot < 0) {
            Slot = i;
        }
    }

    if (Slot < 0) {
        SysReply(&Reply, sizeof(Reply));
        return;
    }

    for (i = 0; i < REGISTRY_NAME_MAX; i++) {
        Services[Slot].Name[i] = Request->Name[i];
    }
    Services[Slot].Endpoint = Request->Endpoint;
    Services[Slot].Shm      = Request->Shm;
    Services[Slot].Owner    = Badge;
    Services[Slot].Used     = 1;

    SysPrint("[init] published '");
    SysPrint(Request->Name);
    SysPrint("' for pid ");
    SysPrintNumber(Badge);
    SysPrint("\n");

    Reply.Status = 0;
    SysReply(&Reply, sizeof(Reply));
}

/* HandleLookup
 * Grant the stored capability back to the caller, badged with the
 * caller's pid so the service can tell its clients apart. The badge is
 * ours to choose - we are the one handing out the capability - and
 * choosing the pid means a server's view of "who" agrees with the
 * kernel's. */
static void HandleLookup(RegistryLookup_t *Request, unsigned Badge)
{
    RegistryReply_t Reply;
    int i;

    Reply.Status = -1;
    Reply.Endpoint = -1;
    Reply.Shm = -1;

    for (i = 0; i < SERVICE_MAX; i++) {
        if (!Services[i].Used || !NameMatches(Services[i].Name, Request->Name)) {
            continue;
        }

        Reply.Endpoint = SysCapGrant((int)Badge, Services[i].Endpoint, Badge);
        if (Reply.Endpoint < 0) {
            break;
        }
        if (Services[i].Shm >= 0) {
            Reply.Shm = SysCapGrant((int)Badge, Services[i].Shm, Badge);
        }
        Reply.Status = 0;
        break;
    }

    SysReply(&Reply, sizeof(Reply));
}

static void HandleList(void)
{
    RegistryReply_t Reply;

    /* Clients need our pid in order to grant us capabilities when they
     * publish, and there is no syscall that reveals who owns an
     * endpoint. Answering it here is the only way they can learn it. */
    Reply.Status   = 0;
    Reply.Endpoint = SysGetPid();
    Reply.Shm      = -1;
    SysReply(&Reply, sizeof(Reply));
}

static void Supervise(const char *Module)
{
    if (ChildCount < SUPERVISED_MAX) {
        Children[ChildCount].Module   = Module;
        Children[ChildCount].Pid      = -1;
        Children[ChildCount].Restarts = 0;
        Children[ChildCount].Failed   = 0;
        ChildCount++;
    }
}

static void Start(Child_t *Child)
{
    int Pid = SysSpawn(Child->Module);

    if (Pid < 0) {
        SysPrint("[init] could not start ");
        SysPrintLine(Child->Module);
        Child->Failed = 1;
        return;
    }
    Child->Pid = Pid;
    SysPrint("[init] started ");
    SysPrint(Child->Module);
    SysPrint(" as pid ");
    SysPrintNumber((unsigned)Pid);
    SysPrint("\n");
}

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    unsigned Ticks = 0;
    int i;

    SysPrint("[init] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Endpoint = SysEndpointCreate();
    if (Endpoint < 0) {
        SysPrintLine("[init] no endpoint");
        SysExit(1);
    }

    /* From here on every process the kernel creates inherits a
     * capability to this endpoint at handle 0. */
    if (SysSetRegistry(Endpoint) != SYSCALL_OK) {
        SysPrintLine("[init] could not become the registry");
        SysExit(1);
    }
    SysPrintLine("[init] registry nominated - naming is now user space");

    Supervise("pci.mod");
    Supervise("serial.mod");

    for (i = 0; i < ChildCount; i++) {
        Start(&Children[i]);
        SysSleep(200);
    }

    SysPrintLine("[init] serving the registry and supervising");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length;

        /* Serve the registry, but not forever: supervision has to get a
         * turn. A registry that only woke on requests would never
         * notice a driver dying in silence. */
        Length = SysRecvTimed(Endpoint, Message, sizeof(Message),
                              &Opcode, &Badge, POLL_MS);

        if (Length >= 0) {
            switch (Opcode) {
                case REGISTRY_PUBLISH:
                    HandlePublish((RegistryPublish_t*)Message, Badge);
                    break;
                case REGISTRY_LOOKUP:
                    HandleLookup((RegistryLookup_t*)Message, Badge);
                    break;
                case REGISTRY_LIST:
                    HandleList();
                    break;
                default:
                    SysReply(0, 0);
                    break;
            }
            continue;
        }

        /* Timed out - do the supervising. */
        Ticks++;
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
            if (Child->Restarts > 5) {
                SysPrint("[init] giving up on ");
                SysPrintLine(Child->Module);
                Child->Failed = 1;
                continue;
            }
            Start(Child);
        }
    }

    return 0;
}
