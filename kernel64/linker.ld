/* linker.ld - where the 64-bit kernel lives.
 *
 * Linked high, loaded low. Every address the code uses is in the top
 * 2 GiB of the address space (the "higher half"), which leaves the whole
 * lower half free for user programs. The bytes themselves are loaded at
 * physical 1 MiB. The bootloader's kernel window maps one onto the other:
 *
 *     virtual 0xFFFFFFFF80000000 + x  ->  physical x
 *
 * AT(...) sets each section's physical (load) address, which becomes
 * p_paddr in the ELF program headers; the bootloader copies each segment
 * to p_paddr and checks p_vaddr == p_paddr + KERNEL_WINDOW.
 */

ENTRY(_start)

KERNEL_WINDOW   = 0xFFFFFFFF80000000;
KERNEL_PHYSICAL = 0x100000;             /* 1 MiB: above the bootloader */

/* Three segments with the permissions each kind of content needs. The
 * bootloader's page tables ignore them for now; the kernel's own will
 * enforce them (no writable code, no executable data). Declaring them
 * explicitly also keeps the ELF headers out of any loaded segment. */
PHDRS
{
    text   PT_LOAD FLAGS(5);    /* R-X */
    rodata PT_LOAD FLAGS(4);    /* R-- */
    data   PT_LOAD FLAGS(6);    /* RW- */
}

SECTIONS
{
    . = KERNEL_WINDOW + KERNEL_PHYSICAL;
    __kernel_start = .;

    /* The __*_start/__*_end markers are page-aligned: the kernel maps
     * each range with its own permissions, and permissions are per page. */
    __text_start = .;
    .text : AT(ADDR(.text) - KERNEL_WINDOW)
    {
        *(.text.entry)          /* _start first, for easy disassembly */
        *(.text .text.*)
    } :text

    . = ALIGN(4K);
    __text_end = .;
    __rodata_start = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_WINDOW)
    {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4K);
    __rodata_end = .;
    __data_start = .;
    .data : AT(ADDR(.data) - KERNEL_WINDOW)
    {
        *(.data .data.*)
        /* The global offset table, if the compiler emits one. Named here
         * so the linker cannot place it after .bss: anything with file
         * contents after .bss would force all of .bss into the file. */
        *(.got .got.*)
    } :data

    /* Part of the data segment, but takes no space in the file: the
     * bootloader zero-fills the difference between p_filesz and p_memsz. */
    .bss : AT(ADDR(.bss) - KERNEL_WINDOW)
    {
        /* The boot stack, with one page below it that the kernel leaves
         * unmapped. Stacks grow down, so overflowing it hits the guard
         * page and faults - instead of silently overwriting whatever
         * would have been below. Listed before *(.bss.*) because the
         * first pattern that matches a section is the one that places it. */
        . = ALIGN(4K);
        __boot_stack_guard = .;
        . += 4K;
        KEEP(*(.bss.boot_stack))
        *(.bss .bss.*)
        *(COMMON)
    } :data

    . = ALIGN(4K);
    __data_end = .;
    __kernel_end = .;

    /* Unwind tables are for panic=unwind, which this kernel never uses. */
    /DISCARD/ :
    {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
    }
}
