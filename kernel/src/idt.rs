// =============================================================================
// NoNameOS — Interrupt Descriptor Table (IDT)
// =============================================================================
//
// IDT — таблица, которая говорит CPU: «при прерывании N вызови функцию X».
//
// Прерывания бывают трёх типов:
//
//   1. ИСКЛЮЧЕНИЯ (Exceptions) — INT 0-31
//      Генерируются самим CPU при ошибках:
//        INT 0  — Division Error (деление на ноль)
//        INT 6  — Invalid Opcode (неизвестная инструкция)
//        INT 8  — Double Fault (ошибка при обработке ошибки)
//        INT 13 — General Protection Fault (нарушение доступа)
//        INT 14 — Page Fault (обращение к незамапленной странице)
//
//   2. АППАРАТНЫЕ ПРЕРЫВАНИЯ (IRQ) — INT 32-47 (после ремаппинга PIC)
//      Генерируются устройствами:
//        INT 32 (IRQ 0)  — Таймер
//        INT 33 (IRQ 1)  — Клавиатура
//        INT 44 (IRQ 12) — Мышь
//
//   3. ПРОГРАММНЫЕ ПРЕРЫВАНИЯ (Software) — INT 48+
//      Вызываются инструкцией `int N`:
//        INT 0x80 — Linux-стиль syscall (можно использовать для нас)
//
// Структура записи IDT (16 байт в x86_64):
//   ┌──────────────┬──────────────────────────────────────────┐
//   │ offset_low   │ биты 0-15 адреса обработчика             │
//   │ selector     │ сегмент кода (0x08 = kernel code)        │
//   │ ist          │ Interrupt Stack Table (0 = не используем) │
//   │ type_attr    │ тип + DPL + Present                      │
//   │ offset_mid   │ биты 16-31 адреса обработчика            │
//   │ offset_high  │ биты 32-63 адреса обработчика            │
//   │ reserved     │ всегда 0                                 │
//   └──────────────┴──────────────────────────────────────────┘
//
// type_attr байт:
//   бит 7    — Present (1 = запись активна)
//   биты 5-6 — DPL (Descriptor Privilege Level, 0 = только ядро)
//   биты 0-3 — Gate Type (0xE = Interrupt Gate, 0xF = Trap Gate)
//
//   Interrupt Gate (0x8E): автоматически очищает IF (запрещает прерывания)
//   Trap Gate (0x8F): НЕ очищает IF (прерывания остаются разрешены)
//
// Когда CPU вызывает обработчик:
//   1. Сохраняет на стек: SS, RSP, RFLAGS, CS, RIP
//   2. Для некоторых исключений добавляет Error Code (32 бита)
//   3. Прыгает на адрес из IDT записи
//   4. Обработчик должен закончиться инструкцией `iretq`
//
// Исключения с Error Code:
//   INT 8  (Double Fault)          — error code всегда 0
//   INT 10 (Invalid TSS)           — selector index
//   INT 11 (Segment Not Present)   — selector index
//   INT 12 (Stack-Segment Fault)   — selector index
//   INT 13 (General Protection)    — selector index или 0
//   INT 14 (Page Fault)            — биты: P, W/R, U/S, RSVD, I/D
//   INT 17 (Alignment Check)       — всегда 0
//   INT 21 (Control Protection)    — varies
//   INT 29 (VMM Communication)     — varies
//   INT 30 (Security Exception)    — varies
// =============================================================================

use core::mem::size_of;
use crate::pic;

/// Количество записей в IDT. x86_64 поддерживает до 256 прерываний.
const IDT_SIZE: usize = 256;

// ---- Структуры ----

/// Одна запись в IDT (16 байт).
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,   // Биты 0-15 адреса обработчика
    selector: u16,     // Селектор сегмента кода (обычно 0x08)
    ist: u8,           // Interrupt Stack Table index (0 = не использовать)
    type_attr: u8,     // Тип гейта + DPL + Present
    offset_mid: u16,   // Биты 16-31 адреса обработчика
    offset_high: u32,  // Биты 32-63 адреса обработчика
    reserved: u32,     // Зарезервировано, всегда 0
}

impl IdtEntry {
    /// Пустая запись (not present). CPU вызовет #GP при попытке использовать.
    const fn empty() -> Self {
        IdtEntry {
            offset_low: 0, selector: 0, ist: 0, type_attr: 0,
            offset_mid: 0, offset_high: 0, reserved: 0,
        }
    }

    /// Установить обработчик для данного прерывания.
    ///
    /// `handler` — адрес функции-обработчика.
    /// `selector` — сегмент кода (0x08 для kernel).
    /// `gate_type` — 0x8E (Interrupt Gate) или 0x8F (Trap Gate).
    fn set_handler(&mut self, handler: u64, selector: u16, gate_type: u8) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.selector = selector;
        self.ist = 0;
        self.type_attr = gate_type;
        self.reserved = 0;
    }
}

/// Указатель на IDT для инструкции LIDT.
#[repr(C, packed)]
struct IdtPointer {
    limit: u16, // Размер IDT в байтах - 1
    base: u64,  // Адрес начала IDT
}

// ---- Глобальная IDT ----

static mut IDT: [IdtEntry; IDT_SIZE] = [IdtEntry::empty(); IDT_SIZE];

// ---- Контекст прерывания ----

/// Стек-фрейм, который CPU сохраняет при вызове прерывания.
/// Эта структура передаётся в наши обработчики из asm-заглушек.
#[repr(C)]
pub struct InterruptFrame {
    // Регистры, сохранённые нашими asm-заглушками (push в обратном порядке)
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,

    // Номер прерывания и error code (подставлены asm-заглушкой)
    pub int_no: u64,
    pub error_code: u64,

    // Сохранено CPU автоматически при входе в прерывание
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

// ---- ASM-заглушки ----
//
// Проблема: CPU при вызове обработчика не сохраняет регистры общего назначения.
// Если обработчик на Rust/C изменит RAX, RBX и т.д. — вернёмся с мусором.
//
// Решение: asm-заглушка (stub) оборачивает каждый обработчик:
//   1. push всех регистров
//   2. вызов Rust-функции (interrupt_dispatch)
//   3. pop всех регистров
//   4. iretq
//
// Для исключений БЕЗ error code — заглушка pushит фейковый 0.
// Для исключений С error code — CPU уже положил его на стек.

core::arch::global_asm!(r#"
.macro isr_no_error num
    .global isr_stub_\num
    isr_stub_\num:
        push 0              // фейковый error code
        push \num           // номер прерывания
        jmp isr_common
.endm

.macro isr_with_error num
    .global isr_stub_\num
    isr_stub_\num:
        // error code уже на стеке (положен CPU)
        push \num           // номер прерывания
        jmp isr_common
.endm

// Общий код для всех прерываний
isr_common:
    // Сохраняем все регистры общего назначения
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    // Первый аргумент (RDI) = указатель на InterruptFrame (вершина стека)
    mov rdi, rsp

    // Вызываем Rust-обработчик
    call interrupt_dispatch

    // Восстанавливаем регистры
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    // Убираем int_no и error_code со стека
    add rsp, 16

    // Возврат из прерывания
    iretq

// --- Исключения CPU (INT 0-31) ---

isr_no_error    0       // #DE  Division Error
isr_no_error    1       // #DB  Debug
isr_no_error    2       //      NMI
isr_no_error    3       // #BP  Breakpoint
isr_no_error    4       // #OF  Overflow
isr_no_error    5       // #BR  Bound Range
isr_no_error    6       // #UD  Invalid Opcode
isr_no_error    7       // #NM  Device Not Available
isr_with_error  8       // #DF  Double Fault
isr_no_error    9       //      Coprocessor Segment Overrun (legacy)
isr_with_error  10      // #TS  Invalid TSS
isr_with_error  11      // #NP  Segment Not Present
isr_with_error  12      // #SS  Stack-Segment Fault
isr_with_error  13      // #GP  General Protection Fault
isr_with_error  14      // #PF  Page Fault
isr_no_error    15      //      Reserved
isr_no_error    16      // #MF  x87 FPU Error
isr_with_error  17      // #AC  Alignment Check
isr_no_error    18      // #MC  Machine Check
isr_no_error    19      // #XM  SIMD Exception
isr_no_error    20      // #VE  Virtualization Exception
isr_with_error  21      // #CP  Control Protection
isr_no_error    22      //      Reserved
isr_no_error    23      //      Reserved
isr_no_error    24      //      Reserved
isr_no_error    25      //      Reserved
isr_no_error    26      //      Reserved
isr_no_error    27      //      Reserved
isr_no_error    28      //      Reserved
isr_no_error    29      //      Reserved
isr_with_error  30      // #SX  Security Exception
isr_no_error    31      //      Reserved

// --- IRQ (INT 32-47) ---

isr_no_error    32      // IRQ 0  — Timer
isr_no_error    33      // IRQ 1  — Keyboard
isr_no_error    34      // IRQ 2  — Cascade
isr_no_error    35      // IRQ 3  — COM2
isr_no_error    36      // IRQ 4  — COM1
isr_no_error    37      // IRQ 5  — LPT2
isr_no_error    38      // IRQ 6  — Floppy
isr_no_error    39      // IRQ 7  — LPT1 / Spurious
isr_no_error    40      // IRQ 8  — RTC
isr_no_error    41      // IRQ 9  — ACPI
isr_no_error    42      // IRQ 10
isr_no_error    43      // IRQ 11
isr_no_error    44      // IRQ 12 — PS/2 Mouse
isr_no_error    45      // IRQ 13 — FPU
isr_no_error    46      // IRQ 14 — Primary ATA
isr_no_error    47      // IRQ 15 — Secondary ATA
"#);

// ---- Внешние символы из asm ----
extern "C" {
    fn isr_stub_0();  fn isr_stub_1();  fn isr_stub_2();  fn isr_stub_3();
    fn isr_stub_4();  fn isr_stub_5();  fn isr_stub_6();  fn isr_stub_7();
    fn isr_stub_8();  fn isr_stub_9();  fn isr_stub_10(); fn isr_stub_11();
    fn isr_stub_12(); fn isr_stub_13(); fn isr_stub_14(); fn isr_stub_15();
    fn isr_stub_16(); fn isr_stub_17(); fn isr_stub_18(); fn isr_stub_19();
    fn isr_stub_20(); fn isr_stub_21(); fn isr_stub_22(); fn isr_stub_23();
    fn isr_stub_24(); fn isr_stub_25(); fn isr_stub_26(); fn isr_stub_27();
    fn isr_stub_28(); fn isr_stub_29(); fn isr_stub_30(); fn isr_stub_31();
    fn isr_stub_32(); fn isr_stub_33(); fn isr_stub_34(); fn isr_stub_35();
    fn isr_stub_36(); fn isr_stub_37(); fn isr_stub_38(); fn isr_stub_39();
    fn isr_stub_40(); fn isr_stub_41(); fn isr_stub_42(); fn isr_stub_43();
    fn isr_stub_44(); fn isr_stub_45(); fn isr_stub_46(); fn isr_stub_47();
}

/// Массив указателей на все asm-заглушки (для удобной регистрации в IDT).
static ISR_STUBS: [unsafe extern "C" fn(); 48] = [
    isr_stub_0,  isr_stub_1,  isr_stub_2,  isr_stub_3,
    isr_stub_4,  isr_stub_5,  isr_stub_6,  isr_stub_7,
    isr_stub_8,  isr_stub_9,  isr_stub_10, isr_stub_11,
    isr_stub_12, isr_stub_13, isr_stub_14, isr_stub_15,
    isr_stub_16, isr_stub_17, isr_stub_18, isr_stub_19,
    isr_stub_20, isr_stub_21, isr_stub_22, isr_stub_23,
    isr_stub_24, isr_stub_25, isr_stub_26, isr_stub_27,
    isr_stub_28, isr_stub_29, isr_stub_30, isr_stub_31,
    isr_stub_32, isr_stub_33, isr_stub_34, isr_stub_35,
    isr_stub_36, isr_stub_37, isr_stub_38, isr_stub_39,
    isr_stub_40, isr_stub_41, isr_stub_42, isr_stub_43,
    isr_stub_44, isr_stub_45, isr_stub_46, isr_stub_47,
];

// ---- Названия исключений (для красивого вывода) ----

static EXCEPTION_NAMES: [&str; 32] = [
    "Division Error",              // 0
    "Debug",                       // 1
    "Non-Maskable Interrupt",      // 2
    "Breakpoint",                  // 3
    "Overflow",                    // 4
    "Bound Range Exceeded",        // 5
    "Invalid Opcode",              // 6
    "Device Not Available",        // 7
    "Double Fault",                // 8
    "Coprocessor Segment Overrun", // 9
    "Invalid TSS",                 // 10
    "Segment Not Present",         // 11
    "Stack-Segment Fault",         // 12
    "General Protection Fault",    // 13
    "Page Fault",                  // 14
    "Reserved",                    // 15
    "x87 FPU Error",               // 16
    "Alignment Check",             // 17
    "Machine Check",               // 18
    "SIMD Exception",              // 19
    "Virtualization Exception",    // 20
    "Control Protection",          // 21
    "Reserved", "Reserved",        // 22-23
    "Reserved", "Reserved",        // 24-25
    "Reserved", "Reserved",        // 26-27
    "Reserved", "Reserved",        // 28-29
    "Security Exception",          // 30
    "Reserved",                    // 31
];

// ---- Инициализация ----

/// Настройка IDT: регистрируем все 48 обработчиков и загружаем таблицу.
pub fn init() {
    unsafe {
        // Регистрируем все 48 заглушек (0-31 исключения, 32-47 IRQ)
        for i in 0..48 {
            let handler_addr = ISR_STUBS[i] as u64;
            // 0x08 = Kernel Code Segment, 0x8E = Interrupt Gate (Ring 0, Present)
            IDT[i].set_handler(handler_addr, 0x08, 0x8E);
        }

        // Загружаем IDT
        let idt_ptr = IdtPointer {
            limit: (size_of::<[IdtEntry; IDT_SIZE]>() - 1) as u16,
            base: (&raw const IDT) as *const IdtEntry as u64,
        };

        core::arch::asm!(
            "lidt [{}]",
            in(reg) &idt_ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

/// Включить аппаратные прерывания (инструкция STI).
/// Вызывать ПОСЛЕ init() PIC и IDT!
pub fn enable_interrupts() {
    unsafe { core::arch::asm!("sti", options(nostack, nomem)); }
}

/// Выключить аппаратные прерывания (инструкция CLI).
pub fn disable_interrupts() {
    unsafe { core::arch::asm!("cli", options(nostack, nomem)); }
}

// ---- Диспетчер прерываний (вызывается из asm) ----

/// Главный обработчик — вызывается из isr_common для КАЖДОГО прерывания.
/// Определяет тип прерывания и вызывает соответствующую логику.
#[no_mangle]
pub extern "C" fn interrupt_dispatch(frame: &InterruptFrame) {
    let int_no = frame.int_no as usize;

    match int_no {
        // --- Исключения CPU (0-31) ---
        0..=31 => {
            handle_exception(frame);
        }

        // --- IRQ 0: Timer ---
        32 => {
            // Тик таймера → планировщик.
            pic::send_eoi(0);
            crate::scheduler::timer_tick();
        }

        // --- IRQ 1: Keyboard ---
        33 => {
            // Читаем скан-код с порта 0x60
            let scancode: u8 = unsafe {
                let val: u8;
                core::arch::asm!(
                    "in al, dx",
                    in("dx") 0x60u16,
                    out("al") val,
                    options(nomem, nostack, preserves_flags)
                );
                val
            };
            crate::keyboard::handle_scancode(scancode);
            pic::send_eoi(1);
        }

        // --- Остальные IRQ (34-47) ---
        34..=47 => {
            let irq = (int_no - 32) as u8;
            pic::send_eoi(irq);
        }

        _ => {}
    }
}

/// Обработка исключений CPU. Выводим информацию и останавливаемся.
fn handle_exception(frame: &InterruptFrame) {
    let int_no = frame.int_no as usize;
    let name = if int_no < 32 { EXCEPTION_NAMES[int_no] } else { "Unknown" };

    crate::println!();
    crate::println!("!!! CPU EXCEPTION: {} (INT {}) !!!", name, int_no);
    crate::println!("  Error Code: {:#x}", frame.error_code);
    crate::println!("  RIP: {:#018x}  CS:  {:#06x}", frame.rip, frame.cs);
    crate::println!("  RSP: {:#018x}  SS:  {:#06x}", frame.rsp, frame.ss);
    crate::println!("  RFLAGS: {:#018x}", frame.rflags);
    crate::println!("  RAX: {:#018x}  RBX: {:#018x}", frame.rax, frame.rbx);
    crate::println!("  RCX: {:#018x}  RDX: {:#018x}", frame.rcx, frame.rdx);
    crate::println!("  RSI: {:#018x}  RDI: {:#018x}", frame.rsi, frame.rdi);
    crate::println!("  RBP: {:#018x}", frame.rbp);

    // Для Page Fault — показываем адрес, который вызвал ошибку (CR2)
    if int_no == 14 {
        let cr2: u64;
        unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)); }
        crate::println!("  CR2 (fault addr): {:#018x}", cr2);
        crate::println!("  Причина: {}{}{}",
            if frame.error_code & 1 == 0 { "page not present" } else { "protection violation" },
            if frame.error_code & 2 != 0 { ", write" } else { ", read" },
            if frame.error_code & 4 != 0 { ", user-mode" } else { ", kernel-mode" },
        );
    }

    crate::println!("System halted.");

    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}
