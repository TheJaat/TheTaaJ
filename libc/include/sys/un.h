/* sys/un.h - addresses of sockets that are names in the file system
 * (AF_UNIX). TheTaaJ's sockets are TCP and UDP only (sys/socket.h);
 * socket(AF_UNIX, ...) fails, and this is only so that programs that
 * mention the type compile. */

#ifndef _SYS_UN_H
#define _SYS_UN_H

#include <sys/socket.h>

#ifdef __cplusplus
extern "C" {
#endif

struct sockaddr_un {
    sa_family_t sun_family; /* AF_UNIX */
    char sun_path[108];
};

#ifdef __cplusplus
}
#endif

#endif
