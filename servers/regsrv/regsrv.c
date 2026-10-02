/* regsrv - the name registry, and nothing else.
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
#define POLL_MS         500

typedef struct _Service {
    char     Name[REGISTRY_NAME_MAX];
    int      Endpoint;          /* handle in THIS process */
    int      Shm;
    unsigned Owner;             /* pid of whoever published it */
    int      Used;
} Service_t;

static Service_t Services[SERVICE_MAX];

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
            /* Taken - but perhaps by a process that no longer exists.
             * A name held forever by a dead publisher is worse than no
             * registry at all: the service can never be restarted, and
             * clients keep receiving capabilities to destroyed objects. */
            if (Services[i].Owner != Badge
                && SysProcessAlive((int)Services[i].Owner)) {
                SysPrint("[init] '");
                SysPrint(Request->Name);
                SysPrint("' is held by pid ");
                SysPrintNumber(Services[i].Owner);
                SysPrintLine(", which is still running");
                SysReply(&Reply, sizeof(Reply));
                return;
            }

            if (Services[i].Owner != Badge) {
                SysPrint("[init] '");
                SysPrint(Request->Name);
                SysPrint("' was held by pid ");
                SysPrintNumber(Services[i].Owner);
                SysPrintLine(", which has exited - reclaiming");
                SysHandleClose(Services[i].Endpoint);
                if (Services[i].Shm >= 0) {
                    SysHandleClose(Services[i].Shm);
                }
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

        /* Do not hand out a capability to a process that has exited.
         * The object behind it is already destroyed, and the client
         * would discover that several calls later with no idea why. */
        if (!SysProcessAlive((int)Services[i].Owner)) {
            SysPrint("[init] '");
            SysPrint(Request->Name);
            SysPrintLine("' is registered to a dead process - dropping it");
            SysHandleClose(Services[i].Endpoint);
            if (Services[i].Shm >= 0) {
                SysHandleClose(Services[i].Shm);
            }
            Services[i].Used = 0;
            break;
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

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];
    int Endpoint;
    int i;

    SysPrint("[regsrv] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Endpoint = SysEndpointCreate();
    if (Endpoint < 0) {
        SysPrintLine("[regsrv] no endpoint");
        SysExit(1);
    }

    /* From here every process the kernel creates inherits a capability
     * to this endpoint at handle 0. This must happen before anything
     * else is started, which is why the registry is the first server
     * and not a job the supervisor also does. */
    if (SysSetRegistry(Endpoint) != SYSCALL_OK) {
        SysPrintLine("[regsrv] could not become the registry");
        SysExit(1);
    }
    SysPrintLine("[regsrv] registry nominated - naming is user space");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecvTimed(Endpoint, Message, sizeof(Message),
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

        /* Reap registrations whose publisher has gone, so a restarted
         * driver can publish its name again. */
        for (i = 0; i < SERVICE_MAX; i++) {
            if (!Services[i].Used
                || SysProcessAlive((int)Services[i].Owner)) {
                continue;
            }
            SysPrint("[regsrv] reclaiming '");
            SysPrint(Services[i].Name);
            SysPrint("' from exited pid ");
            SysPrintNumber(Services[i].Owner);
            SysPrint("\n");
            SysHandleClose(Services[i].Endpoint);
            if (Services[i].Shm >= 0) {
                SysHandleClose(Services[i].Shm);
            }
            Services[i].Used = 0;
        }
    }

    return 0;
}
