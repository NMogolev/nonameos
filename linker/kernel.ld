/* NoNameOS Kernel Linker Script — x86_64, Multiboot2 */

ENTRY(_start)

SECTIONS {
    /* Load kernel at 1 MiB (standard for Multiboot) */
    . = 1M;

    /* Multiboot2 header must be near the start of the binary */
    .multiboot_header : ALIGN(8) {
        *(.multiboot_header)
    }

    /* Code */
    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    /* Read-only data (GDT, strings, etc.) */
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    /* Initialized data */
    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    /* Uninitialized data (BSS) */
    .bss : ALIGN(4K) {
        __bss_start = .;
        *(COMMON)
        *(.bss .bss.*)
        __bss_end = .;
    }

    /* Discard unnecessary sections */
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note .note.*)
    }
}
