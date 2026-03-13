// Global Descriptor Table
//
// The assembly bootstrap already loads a minimal GDT for the transition
// to long mode (64-bit code + data segments). This module reloads the GDT
// from Rust with the same layout, preparing for future expansion:
//   - TSS (Task State Segment) for ring 0 ↔ ring 3 transitions
//   - Per-CPU GDT entries
//   - User-mode code/data segments

use core::mem::size_of;

#[repr(C, packed)]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    granularity: u8,
    base_high: u8,
}

impl GdtEntry {
    const fn null() -> Self {
        GdtEntry {
            limit_low: 0,
            base_low: 0,
            base_mid: 0,
            access: 0,
            granularity: 0,
            base_high: 0,
        }
    }

    const fn new(base: u32, limit: u32, access: u8, flags: u8) -> Self {
        GdtEntry {
            limit_low: (limit & 0xFFFF) as u16,
            base_low: (base & 0xFFFF) as u16,
            base_mid: ((base >> 16) & 0xFF) as u8,
            access,
            granularity: ((limit >> 16) & 0x0F) as u8 | (flags << 4),
            base_high: ((base >> 24) & 0xFF) as u8,
        }
    }
}

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

// Segments:
//   0x00 — Null
//   0x08 — Kernel Code 64-bit (Ring 0)
//   0x10 — Kernel Data         (Ring 0)
//   0x18 — User Code 64-bit   (Ring 3) [reserved]
//   0x20 — User Data           (Ring 3) [reserved]
static GDT: [GdtEntry; 5] = [
    GdtEntry::null(),
    GdtEntry::new(0, 0xFFFFF, 0x9A, 0xA), // 0x08: Kernel Code
    GdtEntry::new(0, 0xFFFFF, 0x92, 0xC), // 0x10: Kernel Data
    GdtEntry::new(0, 0xFFFFF, 0xFA, 0xA), // 0x18: User Code
    GdtEntry::new(0, 0xFFFFF, 0xF2, 0xC), // 0x20: User Data
];

pub fn init() {
    let gdt_ptr = GdtPointer {
        limit: (size_of::<[GdtEntry; 5]>() - 1) as u16,
        base: GDT.as_ptr() as u64,
    };

    unsafe {
        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_ptr,
            options(readonly, nostack, preserves_flags)
        );

        // Reload segment registers with kernel data segment
        core::arch::asm!(
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            options(nostack, preserves_flags)
        );
    }
}
