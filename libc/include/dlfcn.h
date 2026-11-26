/* dlfcn.h - loading code at run time. There is none to load here:
 * every program is linked whole (libc/src/dynamic.rs). dlopen(NULL)
 * gives a handle for the program; dlsym finds nothing; dlerror says why. */
#ifndef _DLFCN_H
#define _DLFCN_H

#ifdef __cplusplus
extern "C" {
#endif

#define RTLD_LAZY   0x001
#define RTLD_NOW    0x002
#define RTLD_NOLOAD 0x004
#define RTLD_GLOBAL 0x100
#define RTLD_LOCAL  0
#define RTLD_NODELETE 0x1000
#define RTLD_DEFAULT ((void *)0)
#define RTLD_NEXT    ((void *)-1l)

typedef struct {
    const char *dli_fname;
    void *dli_fbase;
    const char *dli_sname;
    void *dli_saddr;
} Dl_info;

void *dlopen(const char *name, int flags);
void *dlsym(void *handle, const char *name);
int dlclose(void *handle);
char *dlerror(void);
int dladdr(const void *address, Dl_info *info);

#ifdef __cplusplus
}
#endif

#endif
