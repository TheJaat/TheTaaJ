#ifndef __SHARED_FSSRV_H__
#define __SHARED_FSSRV_H__

#define FSSRV_NAME          "fs"

#define FS_OP_LIST          1   /* FsList_t   -> FsListResult_t  */
#define FS_OP_OPEN          2   /* FsOpen_t   -> FsHandleResult_t */
#define FS_OP_CLOSE         3   /* FsHandle_t -> FsStatus_t      */
#define FS_OP_READ          4   /* FsIo_t     -> FsIoResult_t    */
#define FS_OP_WRITE         5   /* FsIo_t     -> FsIoResult_t    */
#define FS_OP_STAT          6   /* FsPath_t   -> FsStatResult_t  */
#define FS_OP_SYNC          7   /* no payload -> FsStatus_t      */
#define FS_OP_DELETE        8   /* FsPath_t   -> FsStatus_t      */
#define FS_OP_TRUNCATE      9   /* FsTruncate_t -> FsStatus_t    */

#define FS_PATH_MAX         64
#define FS_NAME_MAX         13
#define FS_LIST_MAX         6
#define FS_WINDOW_BYTES     4096

#define FS_OPEN_READ        0x1
#define FS_OPEN_WRITE       0x2
#define FS_OPEN_CREATE      0x4

typedef struct _FsPath   { char Path[FS_PATH_MAX]; } FsPath_t;
typedef struct _FsList   { char Path[FS_PATH_MAX]; } FsList_t;
typedef struct _FsHandle { int Handle; } FsHandle_t;
typedef struct _FsStatus { int Status; } FsStatus_t;

typedef struct _FsOpen {
    char         Path[FS_PATH_MAX];
    unsigned int Flags;
} FsOpen_t;

typedef struct _FsHandleResult {
    int          Status;
    int          Handle;
    unsigned int Size;
} FsHandleResult_t;

/* Data moves through the shared window; a request carries only an
 * offset and a length. */
typedef struct _FsIo {
    int          Handle;
    unsigned int Offset;
    unsigned int Length;
    unsigned int WindowOffset;
} FsIo_t;

typedef struct _FsIoResult {
    int          Status;
    unsigned int Length;
} FsIoResult_t;

typedef struct _FsTruncate {
    char         Path[FS_PATH_MAX];
    unsigned int Size;
} FsTruncate_t;

typedef struct _FsEntry {
    char         Name[FS_NAME_MAX];
    unsigned int Size;
    int          IsDirectory;
} FsEntry_t;

typedef struct _FsListResult {
    int          Status;
    int          Count;
    FsEntry_t    Entries[FS_LIST_MAX];
} FsListResult_t;

typedef struct _FsStatResult {
    int          Status;
    FsEntry_t    Entry;
} FsStatResult_t;

#endif
