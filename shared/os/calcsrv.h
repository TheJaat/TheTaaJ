#ifndef __SHARED_CALCSRV_H__
#define __SHARED_CALCSRV_H__

/* The calc service - the same shape as mathsrv, but over synchronous
 * calls and shared memory instead of pipes, so the two can be compared
 * directly. */

#define CALCSRV_NAME        "calc"

#define CALC_PING           1   /* no payload    -> "pong"          */
#define CALC_ADD            2   /* CalcArgs_t    -> CalcResult_t    */
#define CALC_WHOAMI         3   /* no payload    -> CalcWho_t       */
#define CALC_SUM_SHM        4   /* CalcShm_t     -> CalcResult_t    */

typedef struct _CalcArgs   { int A, B; } CalcArgs_t;
typedef struct _CalcResult { int Value; } CalcResult_t;

/* The badge the server sees, which the client never supplies. */
typedef struct _CalcWho    { unsigned Badge; } CalcWho_t;

/* Bulk: the client fills a shared region and passes only its length.
 * No part of the data crosses a message. */
typedef struct _CalcShm    { unsigned Count; } CalcShm_t;

#endif
