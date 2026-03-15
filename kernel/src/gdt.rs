// =============================================================================
// NoNameOS — Global Descriptor Table + TSS
// =============================================================================
//
// GDT определяет сегменты памяти для разных уровней привилегий.
// В long mode (64-bit) сегментация практически не используется,
// но GDT всё ещё нужна для:
//   1. Разделения kernel (Ring 0) и user (Ring 3) кода
//   2. TSS — для переключения стека при входе из Ring 3 в Ring 0
//   3. syscall/sysret — MSR STAR ссылается на сегменты GDT
//
// Порядок сегментов КРИТИЧЕН для sysret:
//   SYSRET в long mode загружает:
//     CS = STAR[63:48] + 16  (с RPL=3)
//     SS = STAR[63:48] + 8   (с RPL=3)
//   Поэтому user data ДОЛЖЕН идти ПЕРЕД user code.
//
// Layout:
//   0x00 — Null
//   0x08 — Kernel Code 64-bit (Ring 0)
//   0x10 — Kernel Data         (Ring 0)
//   0x18 — User Data           (Ring 3)  ← sysret SS = (0x10+8)|3 = 0x1B
//   0x20 — User Code 64-bit   (Ring 3)  ← sysret CS = (0x10+16)|3 = 0x23
//   0x28 — TSS (low)           (16 bytes, occupies slots 5-6)
//   0x30 — TSS (high)
// =============================================================================

use core::mem::size_of;

// ---- Segment selectors (offsets in GDT) ----

pub const KERNEL_CODE_SEL: u16 = 0x08;
pub const KERNEL_DATA_SEL: u16 = 0x10;
pub const USER_DATA_SEL: u16   = 0x18;  // User Data (RPL=0 here, |3 at use)
pub const USER_CODE_SEL: u16   = 0x20;  // User Code (RPL=0 here, |3 at use)
pub const TSS_SEL: u16         = 0x28;

// With RPL=3 for user segments
pub const USER_DATA_RPL3: u16  = USER_DATA_SEL | 3;  // 0x1B
pub const USER_CODE_RPL3: u16  = USER_CODE_SEL | 3;  // 0x23

#[repr(C, packed)]
#[derive(Clone, Copy)]
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

// ---- TSS (Task State Segment) ----
//
// В long mode TSS используется ТОЛЬКО для:
//   1. RSP0 — стек ядра, на который CPU переключается при прерывании из Ring 3
//   2. IST (Interrupt Stack Table) — отдельные стеки для NMI, double fault и т.д.
//
// Когда пользовательский код (Ring 3) делает syscall или получает прерывание,
// CPU загружает RSP из TSS.RSP0 — без этого kernel stack будет повреждён.

/// Task State Segment для x86_64.
#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    /// RSP для Ring 0 (загружается CPU при переходе из Ring 3 → Ring 0).
    pub rsp0: u64,
    /// RSP для Ring 1 (не используется).
    pub rsp1: u64,
    /// RSP для Ring 2 (не используется).
    pub rsp2: u64,
    reserved1: u64,
    /// IST1..IST7 — отдельные стеки для прерываний.
    pub ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    /// Offset до I/O Permission Bitmap.
    pub iomap_base: u16,
}

impl Tss {
    pub const fn empty() -> Self {
        Tss {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: size_of::<Tss>() as u16,
        }
    }
}

/// Глобальный TSS.
static mut TSS: Tss = Tss::empty();

/// Стек ядра для прерываний из user-space (16 KiB).
static mut KERNEL_INTERRUPT_STACK: [u8; 16384] = [0; 16384];

// ---- GDT таблица ----
// 7 записей (entry 5-6 — TSS занимает 2 слота в 64-bit mode).

static mut GDT: [GdtEntry; 7] = [
    GdtEntry::null(),                          // 0x00: Null
    GdtEntry::new(0, 0xFFFFF, 0x9A, 0xA),     // 0x08: Kernel Code (Ring 0, 64-bit)
    GdtEntry::new(0, 0xFFFFF, 0x92, 0xC),     // 0x10: Kernel Data (Ring 0)
    GdtEntry::new(0, 0xFFFFF, 0xF2, 0xC),     // 0x18: User Data   (Ring 3) ← ПЕРЕД user code!
    GdtEntry::new(0, 0xFFFFF, 0xFA, 0xA),     // 0x20: User Code   (Ring 3, 64-bit)
    GdtEntry::null(),                          // 0x28: TSS low  (заполняется в init)
    GdtEntry::null(),                          // 0x30: TSS high (заполняется в init)
];

/// Установить RSP0 в TSS (вызывается при переключении потоков).
/// Каждый поток имеет свой kernel stack. При переключении в user-space
/// нужно обновить TSS.RSP0, чтобы при прерывании CPU знал куда сохранять контекст.
pub fn set_tss_rsp0(rsp0: u64) {
    unsafe { TSS.rsp0 = rsp0; }
}

/// Получить текущий RSP0 из TSS.
pub fn get_tss_rsp0() -> u64 {
    unsafe { TSS.rsp0 }
}

pub fn init() {
    unsafe {
        // Настраиваем TSS.RSP0 — стек ядра для прерываний из user-space
        let stack_top = (&raw const KERNEL_INTERRUPT_STACK) as *const u8 as u64 + 16384;
        TSS.rsp0 = stack_top;

        // Записываем TSS descriptor в GDT (занимает 2 слота в 64-bit mode).
        // Формат 64-bit TSS descriptor:
        //   Слот 0 (low): стандартный segment descriptor
        //   Слот 1 (high): верхние 32 бита base address + reserved
        let tss_addr = (&raw const TSS) as *const Tss as u64;
        let tss_limit = (size_of::<Tss>() - 1) as u64;

        // Low descriptor
        let low = &mut GDT[5];
        low.limit_low = (tss_limit & 0xFFFF) as u16;
        low.base_low = (tss_addr & 0xFFFF) as u16;
        low.base_mid = ((tss_addr >> 16) & 0xFF) as u8;
        low.access = 0x89; // Present=1, Type=9 (64-bit TSS available)
        low.granularity = ((tss_limit >> 16) & 0x0F) as u8; // flags=0 для TSS
        low.base_high = ((tss_addr >> 24) & 0xFF) as u8;

        // High descriptor (upper 32 bits of base)
        let high = &mut GDT[6];
        high.limit_low = ((tss_addr >> 32) & 0xFFFF) as u16;
        high.base_low = ((tss_addr >> 48) & 0xFFFF) as u16;
        high.base_mid = 0;
        high.access = 0;
        high.granularity = 0;
        high.base_high = 0;

        // Загружаем GDT
        let gdt_ptr = GdtPointer {
            limit: (size_of::<[GdtEntry; 7]>() - 1) as u16,
            base: (&raw const GDT) as *const GdtEntry as u64,
        };

        core::arch::asm!(
            "lgdt [{}]",
            in(reg) &gdt_ptr,
            options(readonly, nostack, preserves_flags)
        );

        // Перезагружаем сегментные регистры с kernel data
        core::arch::asm!(
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            options(nostack, preserves_flags)
        );

        // Загружаем TSS
        core::arch::asm!(
            "ltr ax",
            in("ax") TSS_SEL,
            options(nostack, preserves_flags)
        );
    }
}
