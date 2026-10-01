/* calcsrv - a service over synchronous IPC and shared memory.
 *
 * Compare with mathsrv, which does the same thing over pipes:
 *   - no reply channel: the reply is part of the call
 *   - no framing header: a message has a length because the call does
 *   - no atomic-write flag: nothing is buffered, so nothing interleaves
 *   - callers are identified by a kernel-stamped badge, not a field
 *     they fill in themselves
 */

#include <os/syscall.h>
#include <os/calcsrv.h>

static int Endpoint = -1;
static int ShmHandle = -1;
static unsigned ShmBase = 0;

int ModuleMain(void)
{
    unsigned char Message[IPC_MESSAGE_MAX];

    SysPrint("[calc] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    Endpoint = SysEndpointCreate();
    if (Endpoint < 0) {
        SysPrintLine("[calc] no endpoint");
        SysExit(1);
    }

    /* A region the clients will write into. The server creates it so it
     * owns the frames; clients receive a capability to it. */
    ShmHandle = SysShmCreate(4096);
    if (ShmHandle < 0) {
        SysPrintLine("[calc] no shared memory");
        SysExit(1);
    }
    ShmBase = SysShmMap(ShmHandle);
    if (ShmBase == 0) {
        SysPrintLine("[calc] could not map the region");
        SysExit(1);
    }

    /* Publish the endpoint handle through the old name registry so
     * clients can find it. The registry still deals in pipes, so this
     * advertises the pid and clients ask for a capability by calling
     * SysCapGrant from the server side - see the handshake below. */
    SysPrintLine("[calc] endpoint and 4K region ready");
    SysPrint("[calc] shm mapped at 0x");
    SysPrintNumber(ShmBase);
    SysPrint("\n");
    /* Publishing now means talking to init, not to the kernel. */
    if (SysPublish(CALCSRV_NAME, Endpoint, ShmHandle) != 0) {
        SysPrintLine("[calc] the registry refused the name");
        SysExit(1);
    }
    SysPrintLine("[calc] published as 'calc' via the registry, receiving");

    for (;;) {
        unsigned Opcode = 0, Badge = 0;
        int Length = SysRecv(Endpoint, Message, sizeof(Message),
                             &Opcode, &Badge);

        if (Length < 0) {
            SysPrintLine("[calc] receive failed");
            break;
        }

        switch (Opcode) {
            case CALC_PING:
                SysReply("pong", 5);
                break;

            case CALC_ADD: {
                CalcArgs_t *a = (CalcArgs_t*)Message;
                CalcResult_t r;
                r.Value = (Length >= (int)sizeof(CalcArgs_t))
                        ? (a->A + a->B) : 0;
                SysPrint("[calc] badge ");
                SysPrintNumber(Badge);
                SysPrint(" asked ");
                SysPrintNumber((unsigned)a->A);
                SysPrint(" + ");
                SysPrintNumber((unsigned)a->B);
                SysPrint(" = ");
                SysPrintNumber((unsigned)r.Value);
                SysPrint("\n");
                SysReply(&r, sizeof(r));
                break;
            }

            case CALC_WHOAMI: {
                /* The badge is the kernel's word for who called. The
                 * client sent nothing identifying itself. */
                CalcWho_t w;
                w.Badge = Badge;
                SysReply(&w, sizeof(w));
                break;
            }

            case CALC_SUM_SHM: {
                /* The payload never crossed the message boundary - only
                 * a count did. The data is in memory both processes can
                 * see. */
                CalcShm_t *q = (CalcShm_t*)Message;
                CalcResult_t r;
                unsigned i, n;
                int *Data = (int*)ShmBase;

                n = (Length >= (int)sizeof(CalcShm_t)) ? q->Count : 0;
                if (n > 1024) { n = 1024; }

                r.Value = 0;
                for (i = 0; i < n; i++) {
                    r.Value += Data[i];
                }

                SysPrint("[calc] summed ");
                SysPrintNumber(n);
                SysPrint(" ints from shared memory = ");
                SysPrintNumber((unsigned)r.Value);
                SysPrint("\n");
                SysReply(&r, sizeof(r));
                break;
            }

            default:
                SysReply(0, 0);
                break;
        }
    }

    SysExit(0);
    return 0;
}
