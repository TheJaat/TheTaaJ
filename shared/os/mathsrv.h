#ifndef __SHARED_MATHSRV_H__
#define __SHARED_MATHSRV_H__

/* The interface of the maths service.
 *
 * Shared between the server and every client, for the same reason the
 * syscall numbers are: an opcode is a contract, and two copies of a
 * contract drift. In a full system there would be one of these per
 * service - this is what a driver's interface file looks like. */

#define MATHSRV_NAME        "math"

#define MATH_PING           1   /* no payload  -> "pong"                */
#define MATH_ADD            2   /* MathArgs_t  -> MathResult_t          */
#define MATH_MUL            3   /* MathArgs_t  -> MathResult_t          */
#define MATH_STATS          4   /* no payload  -> MathStats_t           */

typedef struct _MathArgs {
    int A;
    int B;
} MathArgs_t;

typedef struct _MathResult {
    int Value;
} MathResult_t;

typedef struct _MathStats {
    unsigned int Requests;
    unsigned int Clients;
} MathStats_t;

#endif /* __SHARED_MATHSRV_H__ */
