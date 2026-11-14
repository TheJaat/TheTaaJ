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

    . = ALIGN(4K);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) *(.comment) *(.note .note.*) }
}
