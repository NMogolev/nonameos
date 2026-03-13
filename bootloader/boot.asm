; =============================================================================
; NoNameOS — Assembly Bootstrap
; =============================================================================
; Multiboot2 compliant entry point.
; Responsibilities:
;   1. Provide Multiboot2 header for GRUB2
;   2. Set up initial page tables (identity map first 1 GB)
;   3. Enable long mode (x86_64)
;   4. Load 64-bit GDT
;   5. Transfer control to Rust kernel_main()
; =============================================================================

; ---- Multiboot2 Header ----
section .multiboot_header
align 8
header_start:
    dd 0xe85250d6                                       ; Multiboot2 magic
    dd 0                                                ; Architecture: i386
    dd header_end - header_start                        ; Header length
    dd 0x100000000 - (0xe85250d6 + 0 + (header_end - header_start)) ; Checksum

    ; End tag (required)
    dw 0            ; type
    dw 0            ; flags
    dd 8            ; size
header_end:


; ---- BSS: Stack and Page Tables ----
section .bss
align 16
stack_bottom:
    resb 65536      ; 64 KiB kernel stack
stack_top:

align 4096
p4_table:           ; PML4
    resb 4096
p3_table:           ; PDPT
    resb 4096
p2_table:           ; Page Directory
    resb 4096


; ---- 32-bit Entry Point ----
section .text
bits 32

global _start
extern kernel_main

_start:
    ; Set up stack
    mov esp, stack_top

    ; Save multiboot2 info (passed by GRUB in eax/ebx)
    mov edi, eax        ; Multiboot2 magic number  → RDI (1st arg)
    mov esi, ebx        ; Multiboot2 info pointer  → RSI (2nd arg)

    ; Verify multiboot2 magic
    cmp eax, 0x36d76289
    jne .error_no_multiboot

    ; Set up identity-mapped page tables for first 1 GB
    call setup_page_tables

    ; Enable PAE and paging → enter long mode
    call enable_paging

    ; Load 64-bit GDT and far-jump to 64-bit code
    lgdt [gdt64.pointer]
    jmp gdt64.code_segment:long_mode_start

.error_no_multiboot:
    ; Display "ERR:MB" on screen and halt
    mov dword [0xb8000], 0x4f524f45     ; "ER"
    mov dword [0xb8004], 0x4f3a4f52     ; "R:"
    mov dword [0xb8008], 0x4f424f4d     ; "MB"
    hlt


; ---- Page Table Setup (32-bit) ----
setup_page_tables:
    ; P4[0] → P3
    mov eax, p3_table
    or  eax, 0b11               ; present + writable
    mov [p4_table], eax

    ; P3[0] → P2
    mov eax, p2_table
    or  eax, 0b11               ; present + writable
    mov [p3_table], eax

    ; P2[0..511] → 2 MiB huge pages (identity map 0..1 GB)
    mov ecx, 0
.map_p2:
    mov eax, ecx
    shl eax, 21                 ; eax = ecx * 2 MiB (2^21)
    or  eax, 0b10000011         ; present + writable + huge page
    mov [p2_table + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .map_p2

    ret


; ---- Enable Long Mode (32-bit) ----
enable_paging:
    ; Load P4 into CR3
    mov eax, p4_table
    mov cr3, eax

    ; Enable PAE (bit 5 of CR4)
    mov eax, cr4
    or  eax, 1 << 5
    mov cr4, eax

    ; Set Long Mode Enable in EFER MSR
    mov ecx, 0xC0000080         ; IA32_EFER
    rdmsr
    or  eax, 1 << 8             ; LME bit
    wrmsr

    ; Enable paging (bit 31 of CR0)
    mov eax, cr0
    or  eax, 1 << 31
    mov cr0, eax

    ret


; ---- 64-bit GDT (read-only data) ----
section .rodata
align 16
gdt64:
    dq 0                                                ; 0x00: Null
.code_segment: equ $ - gdt64
    dq (1<<43) | (1<<44) | (1<<47) | (1<<53)           ; 0x08: Code — exec, code, present, 64-bit
.data_segment: equ $ - gdt64
    dq (1<<44) | (1<<47) | (1<<41)                     ; 0x10: Data — code/data, present, writable
.pointer:
    dw $ - gdt64 - 1       ; limit
    dq gdt64                ; base


; ---- 64-bit Entry ----
section .text
bits 64

long_mode_start:
    ; Reload data segment selectors
    mov ax, gdt64.data_segment
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Set up 64-bit stack
    mov rsp, stack_top

    ; RDI = multiboot_magic, RSI = multiboot_info (preserved from 32-bit code)
    ; Call Rust kernel entry point
    call kernel_main

    ; Should never return — halt forever
.halt:
    cli
    hlt
    jmp .halt
