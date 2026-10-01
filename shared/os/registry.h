#ifndef __SHARED_REGISTRY_H__
#define __SHARED_REGISTRY_H__

/* The registry protocol.
 *
 * Naming is a user-space service. The kernel hands every new process a
 * capability to the registry's endpoint at HANDLE_REGISTRY and knows
 * nothing further: not the names, not the uniqueness rule, not who owns
 * what, not when an entry should go away. Those are decisions, and
 * decisions belong outside the kernel.
 *
 * Registering is two steps, because a capability cannot travel inside a
 * message payload:
 *
 *   1. the server grants its endpoint to the registry process, which
 *      returns the handle index the registry now holds
 *   2. the server calls REGISTRY_PUBLISH with the name and that index
 *
 * Looking up is the mirror: the registry grants the stored capability
 * back to the caller, badged with the caller's pid, and returns the
 * index in the reply. */

#define REGISTRY_PUBLISH        1   /* RegistryPublish_t -> RegistryReply_t */
#define REGISTRY_LOOKUP         2   /* RegistryLookup_t  -> RegistryReply_t */
#define REGISTRY_LIST           3   /* no payload        -> RegistryList_t  */

#define REGISTRY_NAME_MAX       24

typedef struct _RegistryPublish {
    char         Name[REGISTRY_NAME_MAX];
    int          Endpoint;      /* handle index inside the registry */
    int          Shm;           /* handle index, or -1              */
} RegistryPublish_t;

typedef struct _RegistryLookup {
    char         Name[REGISTRY_NAME_MAX];
} RegistryLookup_t;

typedef struct _RegistryReply {
    int          Status;        /* 0 ok, negative on failure        */
    int          Endpoint;      /* handle index in the CALLER       */
    int          Shm;
} RegistryReply_t;

typedef struct _RegistryList {
    int          Count;
    char         Names[8][REGISTRY_NAME_MAX];
} RegistryList_t;

#endif
