/* user.ld - where a TheTaaJ user program lives in its address space.
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

    . = ALIGN(4K);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) *(.comment) *(.note .note.*) }
}
