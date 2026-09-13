/* mathsrv - a ring 3 RPC service.
 *
 * The shape every driver will take: create a service pipe, publish it,
 * then loop on a blocking listen. While no request is outstanding this
 * process is not scheduled at all. */

#include <os/rpc.h>
#include <os/mathsrv.h>

static unsigned int GlbRequests = 0;
static unsigned int GlbLastClient = 0;
static unsigned int GlbClients = 0;

int ModuleMain(void)
{
    RpcMessage_t Request;
    int Service;

    SysPrint("[math] starting, pid ");
    SysPrintNumber((unsigned)SysGetPid());
    SysPrint("\n");

    /* A server is also a client of other servers, so it needs its own
     * reply channel even though nothing calls out yet. */
    if (RpcInitialize() != SYSCALL_OK) {
        SysPrintLine("[math] could not create a reply channel");
        SysExit(1);
    }

    Service = RpcCreateService(MATHSRV_NAME);
    if (Service < 0) {
        SysPrintLine("[math] could not publish the service");
        SysExit(1);
    }

    SysPrintLine("[math] published as 'math', listening");

    for (;;) {
        if (RpcListen(Service, &Request) < 0) {
            SysPrintLine("[math] listen failed");
            break;
        }

        GlbRequests++;
        if (Request.Header.From != GlbLastClient) {
            GlbLastClient = Request.Header.From;
            GlbClients++;
        }

        switch (Request.Header.Opcode) {
            case MATH_PING: {
                RpcRespond(&Request, "pong", 5);
                break;
            }
            case MATH_ADD:
            case MATH_MUL: {
                MathArgs_t *Args = (MathArgs_t*)Request.Payload;
                MathResult_t Result;

                if (Request.Header.Length < sizeof(MathArgs_t)) {
                    /* A short payload is a malformed request, not a
                     * reason to read past the buffer. */
                    Result.Value = 0;
                }
                else if (Request.Header.Opcode == MATH_ADD) {
                    Result.Value = Args->A + Args->B;
                }
                else {
                    Result.Value = Args->A * Args->B;
                }

                SysPrint("[math] pid ");
                SysPrintNumber(Request.Header.From);
                SysPrint(" asked for ");
                SysPrintNumber((unsigned)Args->A);
                SysPrint(Request.Header.Opcode == MATH_ADD ? " + " : " * ");
                SysPrintNumber((unsigned)Args->B);
                SysPrint(" = ");
                SysPrintNumber((unsigned)Result.Value);
                SysPrint("\n");

                RpcRespond(&Request, &Result, sizeof(Result));
                break;
            }
            case MATH_STATS: {
                MathStats_t Stats;
                Stats.Requests = GlbRequests;
                Stats.Clients  = GlbClients;
                RpcRespond(&Request, &Stats, sizeof(Stats));
                break;
            }
            default: {
                SysPrint("[math] unknown opcode ");
                SysPrintNumber(Request.Header.Opcode);
                SysPrint("\n");
                RpcRespond(&Request, 0, 0);
                break;
            }
        }
    }

    SysExit(0);
    return 0;
}
