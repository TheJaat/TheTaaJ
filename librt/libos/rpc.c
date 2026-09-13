#include <os/rpc.h>

static int RpcReadMessage(int Handle, RpcMessage_t *Message);

static int GlbReplyPipe = -1;
static unsigned int GlbSequence = 0;

/* RpcInitialize */
int RpcInitialize(void)
{
    if (GlbReplyPipe >= 0) {
        return SYSCALL_OK;
    }

    /* The reply channel is atomic too. A server writing a reply while
     * another server writes to the same client would otherwise
     * interleave - rare, but a client with two outstanding calls is a
     * normal thing to build. */
    GlbReplyPipe = SysPipeCreate(sizeof(RpcMessage_t) * 2,
                                 PIPE_FLAG_ATOMIC);
    if (GlbReplyPipe < 0) {
        return GlbReplyPipe;
    }

    return SysCall(SYS_SET_REPLY, (unsigned)GlbReplyPipe, 0, 0);
}

/* RpcCreateService */
int RpcCreateService(const char *Name)
{
    int Pipe = SysPipeCreate(sizeof(RpcMessage_t) * 4, PIPE_FLAG_ATOMIC);
    int Status;

    if (Pipe < 0) {
        return Pipe;
    }

    Status = SysRegisterName(Name, Pipe);
    if (Status != SYSCALL_OK) {
        SysHandleClose(Pipe);
        return Status;
    }
    return Pipe;
}

/* RpcConnect */
int RpcConnect(const char *Name)
{
    return SysLookupName(Name);
}

/* RpcExecute */
int RpcExecute(int Service, unsigned int Opcode,
               const void *In, unsigned int InLength,
               void *Out, unsigned int OutLength)
{
    RpcMessage_t Message;
    RpcMessage_t Reply;
    int Written, Read;

    if (GlbReplyPipe < 0) {
        return SYSCALL_ERROR;       /* RpcInitialize was not called */
    }
    if (InLength > RPC_PAYLOAD_MAX) {
        return SYSCALL_ERROR;
    }

    Message.Header.From     = (unsigned int)SysGetPid();
    Message.Header.Opcode   = Opcode;
    Message.Header.Length   = InLength;
    Message.Header.Sequence = ++GlbSequence;

    if (In != 0 && InLength > 0) {
        SysMemCopy(Message.Payload, In, InLength);
    }

    /* Header and payload in ONE write. Two writes could be separated by
     * another client's message and the server would pair the wrong
     * halves - the failure this whole design exists to prevent. */
    Written = SysPipeWrite(Service, &Message,
                           sizeof(RpcHeader_t) + InLength);
    if (Written <= 0) {
        return SYSCALL_ERROR;
    }

    Read = RpcReadMessage(GlbReplyPipe, &Reply);
    if (Read < (int)sizeof(RpcHeader_t)) {
        return SYSCALL_ERROR;
    }

    if (Out != 0 && OutLength > 0) {
        unsigned int Copy = Reply.Header.Length;
        if (Copy > OutLength) {
            Copy = OutLength;
        }
        SysMemCopy(Out, Reply.Payload, Copy);
        return (int)Copy;
    }

    return (int)Reply.Header.Length;
}

/* RpcReadMessage
 * Reads exactly one message: the fixed header first, then precisely the
 * payload it declares.
 *
 * Reading sizeof(RpcMessage_t) in one go does not work. The pipe is
 * byte-oriented on the read side, so a large read takes everything
 * queued - with two requests waiting it swallows both, and the next
 * listen blocks forever on an empty pipe. Atomic writes keep a message
 * contiguous; stopping at its boundary is the reader's half of the same
 * problem. */
static int RpcReadMessage(int Handle, RpcMessage_t *Message)
{
    int Read;

    Read = SysPipeRead(Handle, &Message->Header, sizeof(RpcHeader_t));
    if (Read < (int)sizeof(RpcHeader_t)) {
        return SYSCALL_ERROR;
    }

    if (Message->Header.Length > RPC_PAYLOAD_MAX) {
        return SYSCALL_ERROR;       /* malformed - do not overrun */
    }

    if (Message->Header.Length > 0) {
        Read = SysPipeRead(Handle, Message->Payload, Message->Header.Length);
        if (Read < (int)Message->Header.Length) {
            return SYSCALL_ERROR;
        }
    }

    return (int)(sizeof(RpcHeader_t) + Message->Header.Length);
}

/* RpcListen */
int RpcListen(int Service, RpcMessage_t *Message)
{
    return RpcReadMessage(Service, Message);
}

/* RpcRespond */
int RpcRespond(const RpcMessage_t *Request, const void *Out,
               unsigned int OutLength)
{
    RpcMessage_t Reply;
    int Handle, Written;

    if (Request == 0 || OutLength > RPC_PAYLOAD_MAX) {
        return SYSCALL_ERROR;
    }

    /* Resolve the caller's channel through the kernel. The server never
     * holds a lasting reference to a client - it opens one per reply and
     * closes it, so a client that has exited cannot be written to by
     * mistake. */
    Handle = SysCall(SYS_OPEN_REPLY, Request->Header.From, 0, 0);
    if (Handle < 0) {
        return Handle;
    }

    Reply.Header.From     = (unsigned int)SysGetPid();
    Reply.Header.Opcode   = Request->Header.Opcode;
    Reply.Header.Length   = OutLength;
    Reply.Header.Sequence = Request->Header.Sequence;

    if (Out != 0 && OutLength > 0) {
        SysMemCopy(Reply.Payload, Out, OutLength);
    }

    Written = SysPipeWrite(Handle, &Reply, sizeof(RpcHeader_t) + OutLength);
    SysHandleClose(Handle);

    return (Written > 0) ? SYSCALL_OK : SYSCALL_ERROR;
}
