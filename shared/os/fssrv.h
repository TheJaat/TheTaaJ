#ifndef __SHARED_FSSRV_H__
#define __SHARED_FSSRV_H__

#define FSSRV_NAME          "fs"

#define FS_OP_LIST          1   /* FsList_t    -> FsListResult_t   */
#define FS_OP_STAT          2   /* FsPath_t    -> FsStat_t         */
#define FS_OP_READ          3   /* FsRead_t    -> FsReadResult_t   */

#define FS_NAME_MAX         13
#define FS_LIST_MAX         8

/* The file service keeps its own shared window, separate from the
 * disk's: a client should not be able to see the raw block cache of a
 * device it has no business touching. */
#define FS_WINDOW_BYTES     4096

typedef struct _FsPath { char Name[FS_NAME_MAX]; } FsPath_t;

typedef struct _FsList {
    unsigned int Cluster;       /* 0 for the root */
} FsList_t;

typedef struct _FsEntry {
    char         Name[FS_NAME_MAX];
    unsigned int Size;
    unsigned int Cluster;
    int          IsDirectory;
} FsEntry_t;

typedef struct _FsListResult {
    int          Status;
    int          Count;
    FsEntry_t    Entries[FS_LIST_MAX];
} FsListResult_t;

typedef struct _FsStat {
    int          Status;
    FsEntry_t    Entry;
} FsStat_t;

typedef struct _FsRead {
    char         Name[FS_NAME_MAX];
    unsigned int Offset;
    unsigned int Length;        /* clamped to FS_WINDOW_BYTES */
} FsRead_t;

typedef struct _FsReadResult {
    int          Status;
    unsigned int Length;        /* bytes placed in the window */
} FsReadResult_t;

#endif
