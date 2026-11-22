/* cxx.ld - user.ld for a C++ program: the same, and the tables that
 * tell how to unwind each function's stack frame, which user.ld throws
 * away. A C++ program needs them: a `throw` walks back up the stack
 * from function to function to the `catch`, and the compiler's tables
 * (.eh_frame) say, for every address, where that function keeps its
 * return address and the registers it saved. libunwind finds them
 * between the symbols defined here (_LIBUNWIND_IS_BAREMETAL, in
 * tools/build-cxx.sh) - and finds the right entry quickly through
 * .eh_frame_hdr, a sorted index the linker builds (--eh-frame-hdr).
 * Without the index (linked without the flag, it is empty) a throw
 * still works: libunwind then reads .eh_frame from the start for every
 * frame, which is slower.
 *
 * user.ld: where a TheTaaJ user program lives in its address space.
 *
 * At 4 MiB, far above the unmapped null region. Each kind of content in
 * its own page-aligned segment, so the kernel can map code read+execute,
 * constants read-only and data read+write - never writable and
 * executable at once (the kernel's loader refuses that outright). */

ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);    /* R-X */
    rodata PT_LOAD FLAGS(4);    /* R-- */
    data   PT_LOAD FLAGS(6);    /* RW- */
    /* No contents: its size is the stack the program wants, which the
     * kernel reads (64 KiB if it is 0). lld leaves it 0 when PHDRS is
     * written out like this, even with -z stack-size; a big program's
     * build writes the size in afterwards (tools/build-llvm.py). */
    stack  PT_GNU_STACK FLAGS(6);
}

SECTIONS
{
    . = 0x400000;
    .text : { *(.text .text.*) } :text

    . = ALIGN(4K);
    .rodata : { *(.rodata .rodata.*) } :rodata

    /* The constructors and destructors (libc/c/atexit.c calls them):
     * arrays of pointers to functions, which nothing writes once the
     * program is linked - so with the constants. KEEP, because nothing
     * refers to them by name, and --gc-sections would throw them out. */
    .init_array : {
        __init_array_start = .;
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
        KEEP(*(.init_array))
        __init_array_end = .;
    } :rodata
    .fini_array : {
        __fini_array_start = .;
        KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*)))
        KEEP(*(.fini_array))
        __fini_array_end = .;
    } :rodata

    /* The unwinding tables: read-only, with the constants too. */
    .eh_frame_hdr : {
        __eh_frame_hdr_start = .;
        *(.eh_frame_hdr)
        __eh_frame_hdr_end = .;
    } :rodata
    .eh_frame : {
        __eh_frame_start = .;
        KEEP(*(.eh_frame))
        __eh_frame_end = .;
    } :rodata

    . = ALIGN(4K);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.comment) *(.note .note.*) }
}
