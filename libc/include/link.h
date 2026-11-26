/* link.h - dl_iterate_phdr: the loaded objects and their program
 * headers. Here there is one, the program itself (libc/src/dynamic.rs). */
#ifndef _LINK_H
#define _LINK_H

#include <elf.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ElfW(type) Elf64_##type

struct dl_phdr_info {
    ElfW(Addr) dlpi_addr;          /* load bias: 0, programs are not moved */
    const char *dlpi_name;         /* "": the program */
    const ElfW(Phdr) *dlpi_phdr;   /* its program headers (a copy) */
    ElfW(Half) dlpi_phnum;
    unsigned long long dlpi_adds;
    unsigned long long dlpi_subs;
    size_t dlpi_tls_modid;         /* 1 if it has thread-locals, else 0 */
    void *dlpi_tls_data;           /* this thread's block of them */
};

/* Calls callback for each loaded object, stopping when it returns
 * non-zero; returns what it last returned. */
int dl_iterate_phdr(int (*callback)(struct dl_phdr_info *info, size_t size, void *data), void *data);

#ifdef __cplusplus
}
#endif

#endif
