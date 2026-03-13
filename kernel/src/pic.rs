// =============================================================================
// NoNameOS — PIC (Programmable Interrupt Controller) 8259
// =============================================================================
//
// Зачем нужен PIC:
//   Железо (клавиатура, таймер, диск) генерирует прерывания (IRQ).
//   PIC собирает эти сигналы и передаёт CPU номер прерывания.
//
// В x86 есть ДВА чипа 8259 (каскадное соединение):
//
//   Master PIC (порты 0x20, 0x21) — обрабатывает IRQ 0-7:
//     IRQ 0 — PIT Timer (системный таймер)
//     IRQ 1 — Keyboard (PS/2 клавиатура)
//     IRQ 2 — Каскад → Slave PIC
//     IRQ 3 — COM2
//     IRQ 4 — COM1
//     IRQ 5 — LPT2
//     IRQ 6 — Floppy
//     IRQ 7 — LPT1 / Spurious
//
//   Slave PIC (порты 0xA0, 0xA1) — обрабатывает IRQ 8-15:
//     IRQ 8  — RTC (Real Time Clock)
//     IRQ 9  — ACPI
//     IRQ 10 — свободен
//     IRQ 11 — свободен
//     IRQ 12 — PS/2 мышь
//     IRQ 13 — FPU
//     IRQ 14 — Primary ATA
//     IRQ 15 — Secondary ATA
//
// Проблема:
//   По умолчанию IRQ 0-7 мапятся на прерывания 8-15.
//   Но прерывания 0-31 зарезервированы CPU для исключений!
//   (0 = Division Error, 8 = Double Fault, 14 = Page Fault...)
//
// Решение:
//   «Ремаппим» PIC — сдвигаем IRQ на прерывания 32+:
//   IRQ 0 → INT 32, IRQ 1 → INT 33, ... IRQ 15 → INT 47
//
// После ремаппинга:
//   INT 0-31   — исключения CPU (Page Fault, GPF и т.д.)
//   INT 32-47  — аппаратные IRQ (таймер, клавиатура и т.д.)
//   INT 48+    — свободны (можно для syscall и т.д.)
//
// После обработки любого IRQ нужно послать EOI (End of Interrupt):
//   • Для IRQ 0-7:  послать EOI в Master PIC
//   • Для IRQ 8-15: послать EOI в Slave PIC И в Master PIC
// =============================================================================

// Порты ввода/вывода PIC
const PIC1_COMMAND: u16 = 0x20;  // Master PIC — командный порт
const PIC1_DATA: u16    = 0x21;  // Master PIC — порт данных
const PIC2_COMMAND: u16 = 0xA0;  // Slave PIC  — командный порт
const PIC2_DATA: u16    = 0xA1;  // Slave PIC  — порт данных

// Команды
const ICW1_INIT: u8  = 0x11;    // Начать инициализацию (ICW4 needed)
const ICW4_8086: u8  = 0x01;    // Режим 8086/88
const PIC_EOI: u8    = 0x20;    // End of Interrupt

/// Смещение: IRQ 0 станет прерыванием 32.
/// Это наш "базовый" номер для аппаратных прерываний.
pub const IRQ_OFFSET: u8 = 32;

// ---- Портовый ввод/вывод ----

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nomem, nostack, preserves_flags)
    );
    val
}

/// Небольшая задержка для PIC (порты медленные).
unsafe fn io_wait() {
    // Запись в неиспользуемый порт 0x80 создаёт задержку ~1 мкс.
    outb(0x80, 0);
}

/// Инициализация и ремаппинг обоих PIC.
///
/// После вызова:
///   IRQ 0-7  → INT 32-39
///   IRQ 8-15 → INT 40-47
///   Все IRQ замаскированы (отключены) — включайте нужные через unmask().
pub fn init() {
    unsafe {
        // Сохраняем текущие маски (какие IRQ были включены)
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        // ICW1: начать инициализацию (каскадный режим, ICW4 будет)
        outb(PIC1_COMMAND, ICW1_INIT); io_wait();
        outb(PIC2_COMMAND, ICW1_INIT); io_wait();

        // ICW2: задаём базовое смещение прерываний
        outb(PIC1_DATA, IRQ_OFFSET);       io_wait(); // Master: IRQ 0-7  → INT 32-39
        outb(PIC2_DATA, IRQ_OFFSET + 8);   io_wait(); // Slave:  IRQ 8-15 → INT 40-47

        // ICW3: настройка каскада
        outb(PIC1_DATA, 0x04); io_wait(); // Master: Slave подключён к IRQ 2 (бит 2)
        outb(PIC2_DATA, 0x02); io_wait(); // Slave:  каскадный ID = 2

        // ICW4: режим 8086
        outb(PIC1_DATA, ICW4_8086); io_wait();
        outb(PIC2_DATA, ICW4_8086); io_wait();

        // Замаскировать все IRQ (0xFF = все биты = все отключены).
        // Будем включать по одному через unmask().
        let _ = mask1;
        let _ = mask2;
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

/// Размаскировать (включить) конкретный IRQ.
///
/// Пример: `unmask(1)` — включить IRQ 1 (клавиатура).
pub fn unmask(irq: u8) {
    let port: u16;
    let irq_bit: u8;

    if irq < 8 {
        port = PIC1_DATA;
        irq_bit = irq;
    } else {
        port = PIC2_DATA;
        irq_bit = irq - 8;
        // Также нужно размаскировать IRQ 2 на Master (каскад к Slave)
        unsafe {
            let mask = inb(PIC1_DATA) & !(1 << 2);
            outb(PIC1_DATA, mask);
        }
    }

    unsafe {
        let mask = inb(port) & !(1 << irq_bit);
        outb(port, mask);
    }
}

/// Замаскировать (отключить) конкретный IRQ.
pub fn mask(irq: u8) {
    let port = if irq < 8 { PIC1_DATA } else { PIC2_DATA };
    let irq_bit = if irq < 8 { irq } else { irq - 8 };

    unsafe {
        let val = inb(port) | (1 << irq_bit);
        outb(port, val);
    }
}

/// Послать EOI (End of Interrupt).
/// Вызывается в конце каждого обработчика прерывания!
///
/// Если не послать EOI, PIC не будет доставлять следующие прерывания.
pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            // Slave PIC — сначала EOI в Slave
            outb(PIC2_COMMAND, PIC_EOI);
        }
        // Всегда EOI в Master
        outb(PIC1_COMMAND, PIC_EOI);
    }
}
