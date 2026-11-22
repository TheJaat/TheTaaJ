/* dirent.h - the names in a directory.
 *
 *   DIR *dir = opendir("docs");
 *   for (struct dirent *e; (e = readdir(dir)) != NULL;)
 *       puts(e->d_name);
 *   closedir(dir);
 *
 * "." and ".." come first, then the directory's own names, which are
 * read all at once by opendir: a name made or removed after that is not
 * seen until the directory is opened again. */

#ifndef _DIRENT_H
#define _DIRENT_H

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct dirent {
    ino_t d_ino;          /* the number stat gives it (st_ino) */
    off_t d_off;
    unsigned short d_reclen;
    unsigned char d_type; /* DT_REG or DT_DIR */
    char d_name[256];
};

#define DT_UNKNOWN 0
#define DT_DIR 4
#define DT_REG 8

typedef struct __taaj_dir DIR;

DIR *opendir(const char *path);
struct dirent *readdir(DIR *dir);
void rewinddir(DIR *dir);
int closedir(DIR *dir);

#ifdef __cplusplus
}
#endif

#endif
