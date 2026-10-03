/* tfstool - make, inspect and populate a TaajFS image from the host.
 *
 * Written before the OS could mount one, deliberately. A filesystem of
 * your own has no fsck to tell you the truth, so the replacement is a
 * second implementation you can run on a real machine with a debugger -
 * and the surest way to have one is to build it first.
 *
 * It shares tfscore.c with the OS, so the format cannot drift. What it
 * does NOT share is the dumper: that walks the raw blocks itself, so a
 * bug in the core shows up as a disagreement rather than being
 * reproduced identically on both sides.
 *
 *   tfstool mkfs <image> [blocks]
 *   tfstool dump <image>
 *   tfstool ls   <image>
 *   tfstool put  <image> <name> <file>
 *   tfstool get  <image> <name>
 *   tfstool rm   <image> <name>
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "tfscore.h"

static FILE *Image;

static int ReadBlock(void *C, unsigned Block, void *Buffer)
{
    (void)C;
    if (fseek(Image, (long)Block * TFS_BLOCK_SIZE, SEEK_SET) != 0) return -1;
    if (fread(Buffer, 1, TFS_BLOCK_SIZE, Image) != TFS_BLOCK_SIZE) {
        /* Past the end of a sparse file reads as zeros, which is what a
         * fresh image looks like. */
        memset(Buffer, 0, TFS_BLOCK_SIZE);
    }
    return 0;
}

static int WriteBlock(void *C, unsigned Block, const void *Buffer)
{
    (void)C;
    if (fseek(Image, (long)Block * TFS_BLOCK_SIZE, SEEK_SET) != 0) return -1;
    if (fwrite(Buffer, 1, TFS_BLOCK_SIZE, Image) != TFS_BLOCK_SIZE) return -1;
    fflush(Image);
    return 0;
}

/* ---- the independent reader -------------------------------------- */

static unsigned RawChecksum(const TfsSuper_t *S)
{
    const unsigned char *b = (const unsigned char*)S;
    unsigned Sum = 0, i;
    unsigned Off = (unsigned)((const unsigned char*)&S->Checksum
                            - (const unsigned char*)S);
    for (i = 0; i < sizeof(TfsSuper_t); i++) {
        if (i >= Off && i < Off + 4) continue;
        Sum += b[i];
        Sum = (Sum << 1) | (Sum >> 31);
    }
    return Sum;
}

static int Dump(void)
{
    unsigned char Block[TFS_BLOCK_SIZE];
    TfsSuper_t Slot[2];
    int Valid[2], slot, Live = -1;
    unsigned DirBlock, Guard = 0, Files = 0, Blocks = 0;

    printf("superblocks:\n");
    for (slot = 0; slot < 2; slot++) {
        TfsSuper_t *S = (TfsSuper_t*)Block;
        ReadBlock(0, (unsigned)slot, Block);
        Slot[slot] = *S;
        Valid[slot] = (S->Magic == TFS_MAGIC
                    && S->Version == TFS_VERSION
                    && S->Checksum == RawChecksum(S));
        printf("  slot %c: %s", 'A' + slot,
               Valid[slot] ? "valid  " : "INVALID");
        if (S->Magic == TFS_MAGIC) {
            printf("  generation %-6u root block %-6u total %u",
                   S->Generation, S->RootDir, S->TotalBlocks);
            if (!Valid[slot]) {
                printf("   (checksum %08x, computed %08x)",
                       S->Checksum, RawChecksum(S));
            }
        }
        printf("\n");
    }

    for (slot = 0; slot < 2; slot++) {
        if (!Valid[slot]) continue;
        if (Live < 0 || Slot[slot].Generation > Slot[Live].Generation) Live = slot;
    }
    if (Live < 0) { printf("\nno valid superblock - the volume is unmountable\n"); return 1; }

    printf("\nlive: slot %c, generation %u\n", 'A' + Live, Slot[Live].Generation);

    printf("\ntree:\n");
    DirBlock = Slot[Live].RootDir;
    while (DirBlock != 0) {
        TfsDirBlock_t *Dir = (TfsDirBlock_t*)Block;
        unsigned i;

        if (++Guard > 64) { printf("  (directory chain loops)\n"); return 1; }
        ReadBlock(0, DirBlock, Block);
        if (Dir->Magic != TFS_MAGIC) {
            printf("  block %u is not a directory block\n", DirBlock);
            return 1;
        }
        printf("  dir block %u: %u entries, next %u\n",
               DirBlock, Dir->Count, Dir->Next);
        Blocks++;

        for (i = 0; i < Dir->Count && i < TFS_DIR_ENTRIES; i++) {
            unsigned char IB[TFS_BLOCK_SIZE];
            TfsInode_t *N = (TfsInode_t*)IB;

            if (Dir->Entries[i].Inode == 0) continue;
            ReadBlock(0, Dir->Entries[i].Inode, IB);
            printf("    %-20s inode %-5u size %-7u blocks %u%s\n",
                   Dir->Entries[i].Name, Dir->Entries[i].Inode,
                   Dir->Entries[i].Size, N->BlockCount,
                   (N->Magic == TFS_MAGIC) ? "" : "   [BAD INODE MAGIC]");
            if (N->Size != Dir->Entries[i].Size) {
                printf("      [MISMATCH] inode says %u, directory says %u\n",
                       N->Size, Dir->Entries[i].Size);
            }
            Files++;
            Blocks += 1 + N->BlockCount;
        }
        DirBlock = Dir->Next;
    }

    printf("\n%u files, %u blocks reachable, %u of %u free\n",
           Files, Blocks + 2, Slot[Live].TotalBlocks - (Blocks + 2),
           Slot[Live].TotalBlocks);
    return 0;
}

int main(int argc, char **argv)
{
    TfsVolume_t V;
    int Status;

    if (argc < 3) {
        printf("usage: tfstool mkfs|dump|ls|put|get|rm <image> [args]\n");
        return 1;
    }

    if (strcmp(argv[1], "mkfs") == 0) {
        unsigned Blocks = (argc > 3) ? (unsigned)atoi(argv[3]) : 4096;
        Image = fopen(argv[2], "w+b");
        if (!Image) { printf("cannot create %s\n", argv[2]); return 1; }
        /* Lay the file out so later seeks do not read past the end. */
        {
            unsigned char Zero[TFS_BLOCK_SIZE];
            unsigned i;
            memset(Zero, 0, sizeof(Zero));
            for (i = 0; i < Blocks; i++) fwrite(Zero, 1, sizeof(Zero), Image);
        }
        Status = TfsFormat(ReadBlock, WriteBlock, 0, Blocks);
        printf("mkfs: %s, %u blocks of %u bytes -> %s\n",
               TfsError(Status), Blocks, TFS_BLOCK_SIZE, argv[2]);
        fclose(Image);
        return Status != TFS_OK;
    }

    Image = fopen(argv[2], "r+b");
    if (!Image) { printf("cannot open %s\n", argv[2]); return 1; }

    if (strcmp(argv[1], "dump") == 0) { int r = Dump(); fclose(Image); return r; }

    Status = TfsMount(&V, ReadBlock, WriteBlock, 0);
    if (Status != TFS_OK) {
        printf("mount: %s\n", TfsError(Status));
        fclose(Image);
        return 1;
    }

    if (strcmp(argv[1], "ls") == 0) {
        TfsFile_t Files[64];
        int n = TfsList(&V, Files, 64), i;
        for (i = 0; i < n; i++)
            printf("  %-24s %8u\n", Files[i].Name, Files[i].Size);
        printf("  %d files, %u blocks free\n", n, TfsFreeBlocks(&V));
    }
    else if (strcmp(argv[1], "put") == 0 && argc >= 5) {
        FILE *In = fopen(argv[4], "rb");
        static unsigned char Buffer[TFS_MAX_FILE_SIZE];
        size_t n;

        if (!In) { printf("cannot read %s\n", argv[4]); fclose(Image); return 1; }
        n = fread(Buffer, 1, sizeof(Buffer), In);
        fclose(In);

        Status = TfsCreate(&V, argv[3]);
        if (Status != TFS_OK && Status != TFS_ERR_EXISTS) {
            printf("create: %s\n", TfsError(Status)); fclose(Image); return 1;
        }
        Status = TfsWriteFile(&V, argv[3], 0, Buffer, (unsigned)n);
        printf("put: %u bytes -> %s (%s)\n", (unsigned)n, argv[3],
               Status >= 0 ? "ok" : TfsError(Status));
    }
    else if (strcmp(argv[1], "get") == 0 && argc >= 4) {
        TfsFile_t F;
        static unsigned char Buffer[TFS_MAX_FILE_SIZE];
        int n;

        if (TfsLookup(&V, argv[3], &F) != TFS_OK) {
            printf("not found\n"); fclose(Image); return 1;
        }
        n = TfsRead(&V, &F, 0, Buffer, F.Size);
        fwrite(Buffer, 1, (size_t)(n > 0 ? n : 0), stdout);
    }
    else if (strcmp(argv[1], "rm") == 0 && argc >= 4) {
        Status = TfsDelete(&V, argv[3]);
        printf("rm: %s\n", TfsError(Status));
    }
    else {
        printf("unknown command\n");
    }

    fclose(Image);
    return 0;
}
