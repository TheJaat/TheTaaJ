#ifndef __SHARED_TFS_H__
#define __SHARED_TFS_H__

/* TaajFS - a copy-on-write filesystem.
 *
 * The one thing FAT cannot do is survive an interrupted write. Its
 * structures are mutated in place, so a crash between updating the FAT
 * and updating the directory entry leaves a volume that needs a checker
 * to repair. No ordering trick fixes that; it is inherent to the format.
 *
 * Copy-on-write fixes it completely, and does so by being SIMPLER rather
 * than more clever:
 *
 *   - a live block is never overwritten
 *   - a change writes new blocks, then new blocks for everything that
 *     pointed at them, up to the root
 *   - the commit is one write of one superblock
 *
 * A torn write therefore leaves the previous tree entirely intact, and
 * there is no log, no replay, and no ordering rules to get subtly
 * wrong. The cost is that every change rewrites a path to the root, and
 * that free space has to be worked out rather than tracked.
 *
 * There is deliberately no allocation bitmap on disk. The committed tree
 * defines exactly what is in use; everything else is free. That is
 * recomputed at mount by walking the tree, which removes an entire class
 * of bug - an allocator that disagrees with reality - and is cheap at
 * these sizes. */

#define TFS_MAGIC           0x5346414Au  /* 'JAFS' little-endian  */
#define TFS_VERSION         1
#define TFS_BLOCK_SIZE      4096
#define TFS_SECTORS_PER_BLOCK (TFS_BLOCK_SIZE / 512)

#define TFS_NAME_MAX        28
#define TFS_MAX_BLOCKS      65536       /* 256 MB at 4 KB blocks */

/* Two superblock slots, written alternately.
 *
 * The commit is a single block write, and a single block write can still
 * be torn. Alternating means the previous superblock is always intact
 * somewhere, and the checksum says which of the two is whole. */
#define TFS_SUPER_A         0
#define TFS_SUPER_B         1
#define TFS_FIRST_DATA      2

#define TFS_TYPE_FILE       1
#define TFS_TYPE_DIR        2

/* The superblock must stay inside ONE 512-byte sector.
 *
 * A 4 KB block write is eight sector writes, and a power loss can leave
 * some done and some not. Because every field lives in sector 0, the
 * only two outcomes are "sector 0 landed" - in which case every field is
 * from the new write and the checksum agrees - or "it did not", leaving
 * the old superblock entirely. There is no outcome where half the
 * fields are new.
 *
 * That is why the commit is atomic without any further machinery, and
 * it stops being true the moment this structure grows past 512 bytes.
 * It is 36 bytes; keep it small. */
typedef struct _TfsSuper {
    unsigned int Magic;
    unsigned int Version;
    unsigned int BlockSize;
    unsigned int TotalBlocks;

    /* Higher wins, provided the checksum is good. Monotonic across
     * commits, so there is never ambiguity about which slot is newer. */
    unsigned int Generation;

    unsigned int RootDir;       /* block holding the root directory */
    unsigned int Reserved[2];
    unsigned int Checksum;      /* additive, computed with this zeroed */
} TfsSuper_t;

typedef struct _TfsDirEntry {
    char         Name[TFS_NAME_MAX];
    unsigned int Inode;         /* block holding the inode */
    unsigned int Size;
    unsigned int Type;
} TfsDirEntry_t;                /* 40 bytes */

#define TFS_DIR_HEADER      16
#define TFS_DIR_ENTRIES     ((TFS_BLOCK_SIZE - TFS_DIR_HEADER) / 40)

typedef struct _TfsDirBlock {
    unsigned int  Magic;
    unsigned int  Count;
    unsigned int  Next;         /* 0 = last block of the directory */
    unsigned int  Reserved;
    TfsDirEntry_t Entries[TFS_DIR_ENTRIES];
} TfsDirBlock_t;

#define TFS_INODE_HEADER    16
#define TFS_INODE_BLOCKS    ((TFS_BLOCK_SIZE - TFS_INODE_HEADER) / 4)

typedef struct _TfsInode {
    unsigned int Magic;
    unsigned int Size;
    unsigned int Type;
    unsigned int BlockCount;
    unsigned int Blocks[TFS_INODE_BLOCKS];
} TfsInode_t;                   /* direct blocks only: 4 MB per file */

#define TFS_MAX_FILE_SIZE   (TFS_INODE_BLOCKS * TFS_BLOCK_SIZE)

#endif
