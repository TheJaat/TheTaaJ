/* Client side of the registry protocol.
 *
 * These look like system calls and are not. They are ordinary
 * synchronous calls on HANDLE_REGISTRY, which the kernel handed us at
 * startup. The kernel does not know that names exist. */

#include <os/syscall.h>
#include <os/registry.h>

/* SysPublish
 * Two steps, because a capability cannot travel inside a message.
 *
 *   1. grant the endpoint to the registry process, which tells us the
 *      handle index the registry now holds
 *   2. call the registry with the name and that index
 *
 * The grant has to come first: the registry cannot store a capability
 * it has not been given, and the message can only carry the index. */
int SysPublish(const char *Name, int Endpoint, int Shm)
{
    RegistryPublish_t Request;
    RegistryReply_t Reply;
    int RegistryPid;
    int TheirEndpoint, TheirShm = -1;
    unsigned i;

    RegistryPid = SysRegistryPid();
    if (RegistryPid < 0) {
        return SYSCALL_NOTFOUND;
    }

    TheirEndpoint = SysCapGrant(RegistryPid, Endpoint, 0);
    if (TheirEndpoint < 0) {
        return TheirEndpoint;
    }
    if (Shm >= 0) {
        TheirShm = SysCapGrant(RegistryPid, Shm, 0);
    }

    for (i = 0; i < REGISTRY_NAME_MAX; i++) {
        Request.Name[i] = (i < SysStringLength(Name)) ? Name[i] : '\0';
    }
    Request.Endpoint = TheirEndpoint;
    Request.Shm      = TheirShm;

    Reply.Status = SYSCALL_ERROR;
    if (SysCallTimed(HANDLE_REGISTRY, REGISTRY_PUBLISH,
            &Request, sizeof(Request), &Reply, sizeof(Reply), 2000)
        < (int)sizeof(Reply)) {
        return SYSCALL_ERROR;
    }

    return Reply.Status;
}

/* SysLookup
 * The mirror: the registry grants the stored capability back to us and
 * the reply carries the handle index in OUR table. */
int SysLookup(const char *Name, int *Endpoint, int *Shm)
{
    RegistryLookup_t Request;
    RegistryReply_t Reply;
    unsigned i;

    for (i = 0; i < REGISTRY_NAME_MAX; i++) {
        Request.Name[i] = (i < SysStringLength(Name)) ? Name[i] : '\0';
    }

    Reply.Status = SYSCALL_ERROR;
    Reply.Endpoint = -1;
    Reply.Shm = -1;

    if (SysCallTimed(HANDLE_REGISTRY, REGISTRY_LOOKUP,
            &Request, sizeof(Request), &Reply, sizeof(Reply), 2000)
        < (int)sizeof(Reply)) {
        return SYSCALL_ERROR;
    }

    if (Reply.Status == 0) {
        if (Endpoint != 0) { *Endpoint = Reply.Endpoint; }
        if (Shm != 0)      { *Shm = Reply.Shm; }
    }
    return Reply.Status;
}

/* SysRegistryPid
 * The registry answers REGISTRY_LIST with its own pid in Count's place
 * only in the degenerate case; instead we ask the kernel, because the
 * registry endpoint's owner is a fact the kernel knows and we do not.
 *
 * There is no syscall for "who owns this endpoint" - deliberately, it
 * would leak the shape of another process's world. So the registry
 * publishes its pid the only way it can: by answering a call. */
static int GlbRegistryPid = -1;

int SysRegistryPid(void)
{
    RegistryReply_t Reply;

    if (GlbRegistryPid >= 0) {
        return GlbRegistryPid;
    }

    Reply.Status = SYSCALL_ERROR;
    Reply.Endpoint = -1;
    if (SysCallTimed(HANDLE_REGISTRY, REGISTRY_LIST, 0, 0,
            &Reply, sizeof(Reply), 2000) < (int)sizeof(Reply)) {
        return SYSCALL_NOTFOUND;
    }
    if (Reply.Status != 0) {
        return SYSCALL_NOTFOUND;
    }

    GlbRegistryPid = Reply.Endpoint;   /* the registry's own pid */
    return GlbRegistryPid;
}
