#ifndef __TFSCORE_H__
#define __TFSCORE_H__

#include <os/tfs.h>

typedef int (*TfsRead_t)(void *Context, unsigned Block, void *Buffer);
typedef int (*TfsWrite_t)(void *Context, unsigned Block, const void *Buffer);

typedef struct _TfsVolume {
    TfsRead_t      Read;
    TfsWrite_t     Write;
    void          *Context;

    TfsSuper_t     Super;
    int            SuperSlot;       /* which slot the live one is in */
    int            Mounted;

    /* In-memory free map, rebuilt from the committed tree at mount and
     * after every commit. Nothing on disk tracks allocation. */
    unsigned char  Used[TFS_MAX_BLOCKS / 8];
    unsigned int   UsedCount;
} TfsVolume_t;

typedef struct _TfsFile {
    char         Name[TFS_NAME_MAX];
    unsigned int Inode;
    unsigned int Size;
    unsigned int Type;
} TfsFile_t;

#define TFS_OK              0
#define TFS_ERR_IO         -1
#define TFS_ERR_MAGIC      -2
#define TFS_ERR_NOSUPER    -3   /* neither slot is valid */
#define TFS_ERR_FULL       -4
#define TFS_ERR_NOTFOUND   -5
#define TFS_ERR_EXISTS     -6
#define TFS_ERR_NAME       -7
#define TFS_ERR_TOOBIG     -8
#define TFS_ERR_READONLY   -9

const char *TfsError(int Code);

int  TfsFormat(TfsRead_t, TfsWrite_t, void *Context, unsigned TotalBlocks);
int  TfsMount(TfsVolume_t *, TfsRead_t, TfsWrite_t, void *Context);

int  TfsList(TfsVolume_t *, TfsFile_t *Files, int Max);
int  TfsLookup(TfsVolume_t *, const char *Name, TfsFile_t *);
int  TfsRead(TfsVolume_t *, const TfsFile_t *, unsigned Offset,
             void *Buffer, unsigned Length);

/* Every one of these commits before returning, so a successful call has
 * reached the disk and a crash during one leaves the previous state. */
int  TfsCreate(TfsVolume_t *, const char *Name);
int  TfsWriteFile(TfsVolume_t *, const char *Name, unsigned Offset,
                  const void *Buffer, unsigned Length);
int  TfsDelete(TfsVolume_t *, const char *Name);

unsigned TfsFreeBlocks(TfsVolume_t *);

#endif
