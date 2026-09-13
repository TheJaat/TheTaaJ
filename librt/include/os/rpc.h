#ifndef __LIBRT_RPC_H__
#define __LIBRT_RPC_H__

#include <os/syscall.h>

/* Request/reply over pipes.
 *
 * A pipe carries bytes. A service call is a request with an opcode,
 * arguments and a reply, so three things have to be added on top:
 *
 *   framing     the server must know where a message ends
 *   atomicity   two clients writing must not interleave
 *   addressing  the reply must reach the caller and nobody else
 *
 * Framing is the header below. Atomicity is PIPE_FLAG_ATOMIC, which
 * makes the kernel write a message whole or not at all. Addressing is
 * the caller's process id plus SYS_OPEN_REPLY, because a handle number
 * is meaningless outside the process that owns it. */

#define RPC_PAYLOAD_MAX     192

typedef struct _RpcHeader {
    unsigned int From;          /* caller's process id */
    unsigned int Opcode;
    unsigned int Length;        /* payload bytes following the header */
    unsigned int Sequence;
} RpcHeader_t;

typedef struct _RpcMessage {
    RpcHeader_t  Header;
    unsigned char Payload[RPC_PAYLOAD_MAX];
} RpcMessage_t;

/* RpcInitialize
 * Creates this process's reply channel and nominates it. Must be called
 * before RpcExecute. Returns SYSCALL_OK or a negative error. */
int RpcInitialize(void);

/* RpcCreateService
 * Creates an atomic request pipe and publishes it under <Name>.
 * Returns the pipe handle, or negative. */
int RpcCreateService(const char *Name);

/* RpcConnect
 * Looks up a service by name. Returns a handle to its request pipe. */
int RpcConnect(const char *Name);

/* RpcExecute
 * Sends a request and blocks for the reply. Returns the number of
 * payload bytes in the reply, or negative. */
int RpcExecute(int Service, unsigned int Opcode,
               const void *In, unsigned int InLength,
               void *Out, unsigned int OutLength);

/* RpcListen
 * Blocks for the next request on a service pipe. */
int RpcListen(int Service, RpcMessage_t *Message);

/* RpcRespond
 * Replies to a request. Resolves the caller's reply channel from the
 * header's From field. */
int RpcRespond(const RpcMessage_t *Request, const void *Out,
               unsigned int OutLength);

#endif /* __LIBRT_RPC_H__ */
