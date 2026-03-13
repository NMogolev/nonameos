// =============================================================================
// NoNameOS — Точка входа ядра
// =============================================================================
//
// Порядок загрузки:
//   1. BIOS/UEFI → GRUB2 → boot.asm (32-bit protected mode)
//   2. boot.asm: page tables → long mode → вызов kernel_main()
//   3. kernel_main(): инициализация подсистем → ожидание ввода
//
// Модули ядра:
//   vga      — текстовый вывод 80×25 (VGA text mode)
//   gdt      — Global Descriptor Table (сегменты памяти)
//   idt      — Interrupt Descriptor Table (обработчики прерываний)
//   pic      — Programmable Interrupt Controller (маршрутизация IRQ)
//   serial   — COM1 порт для отладки через QEMU
//   memory   — физический аллокатор + виртуальная память (paging)
//   keyboard — PS/2 клавиатура
//   task     — структуры процессов/потоков (планировщик — будущее)
//   ipc      — межпроцессное взаимодействие (message passing)
// =============================================================================

#![no_std]
#![no_main]

mod vga;
mod gdt;
mod idt;
pub mod pic;
mod serial;
mod memory;
pub mod keyboard;
mod task;
mod ipc;

use core::panic::PanicInfo;

// ---- Макросы вывода (доступны из всех модулей через crate::println!) ----

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        $crate::vga::WRITER.lock().write_fmt(format_args!($($arg)*)).unwrap();
    });
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

// ---- Символы из линкер-скрипта ----
// Линкер предоставляет адреса начала/конца секций ядра.
// Мы используем их, чтобы пометить память ядра как «занятую».
extern "C" {
    static __bss_end: u8;
}

/// Точка входа ядра.
///
/// Вызывается из boot.asm после перехода в 64-bit long mode.
/// Аргументы передаются по System V AMD64 ABI:
///   RDI = multiboot_magic (должен быть 0x36d76289)
///   RSI = multiboot_info  (адрес структуры Multiboot2 info)
#[no_mangle]
pub extern "C" fn kernel_main(multiboot_magic: u64, multiboot_info: u64) -> ! {
    // =========================================================================
    // ЭТАП 1: Базовый вывод
    // =========================================================================
    // Первым делом — VGA, чтобы видеть что происходит.
    vga::clear_screen();

    println!("==========================================");
    println!("  NoNameOS v0.1.0 — Microkernel (Rust)");
    println!("  GPL-3.0 | x86_64 | Multiboot2");
    println!("==========================================");
    println!();

    // Проверяем, что нас загрузил Multiboot2-совместимый загрузчик
    if multiboot_magic == 0x36d76289 {
        println!("[OK] Multiboot2 magic verified");
        println!("     Info struct at: {:#x}", multiboot_info);
    } else {
        println!("[FAIL] Bad Multiboot2 magic: {:#x}", multiboot_magic);
        println!("       Expected: 0x36d76289");
        println!("       System cannot continue.");
        halt();
    }

    // =========================================================================
    // ЭТАП 2: Отладочный serial порт
    // =========================================================================
    serial::init();
    println!("[OK] Serial port (COM1) @ 115200 baud");

    // =========================================================================
    // ЭТАП 3: GDT — сегменты памяти
    // =========================================================================
    // boot.asm уже загрузил базовый GDT. Здесь перезагружаем из Rust
    // с расширенными сегментами (kernel + user mode).
    gdt::init();
    println!("[OK] GDT reloaded (kernel + user segments)");

    // =========================================================================
    // ЭТАП 4: IDT + PIC — прерывания
    // =========================================================================
    // Сначала PIC (ремаппинг IRQ 0-15 → INT 32-47),
    // потом IDT (регистрация обработчиков для исключений и IRQ).
    pic::init();
    println!("[OK] PIC remapped (IRQ 0-15 -> INT 32-47)");

    idt::init();
    println!("[OK] IDT loaded (32 exceptions + 16 IRQ handlers)");

    // =========================================================================
    // ЭТАП 5: Память
    // =========================================================================
    // Инициализируем физический аллокатор.
    // Пока хардкодим 64 МиБ RAM (в будущем — читаем из Multiboot2 memory map).
    let assumed_memory: usize = 64 * 1024 * 1024; // 64 MiB
    let kernel_start: usize = 0x100000; // 1 MiB (куда GRUB загружает ядро)
    let kernel_end: usize = unsafe { &__bss_end as *const u8 as usize };

    memory::phys::init(assumed_memory, kernel_start, kernel_end);

    let free_kb = memory::phys::free_memory_kb();
    let total_kb = memory::phys::total_memory_kb();
    println!("[OK] Physical memory: {} KiB free / {} KiB total", free_kb, total_kb);
    println!("     Kernel: {:#x} — {:#x} ({} KiB)",
        kernel_start, kernel_end, (kernel_end - kernel_start) / 1024);

    // =========================================================================
    // ЭТАП 6: Клавиатура
    // =========================================================================
    // Размаскируем IRQ 1 (клавиатура) в PIC, включаем прерывания.
    pic::unmask(1);   // IRQ 1 = PS/2 Keyboard
    pic::unmask(0);   // IRQ 0 = Timer (для будущего планировщика)
    idt::enable_interrupts();
    println!("[OK] Interrupts enabled (keyboard + timer)");

    // =========================================================================
    // ЭТАП 7: Готовность
    // =========================================================================
    println!();
    println!("==========================================");
    println!("  NoNameOS kernel initialized.");
    println!("  Type something — keyboard is active!");
    println!("==========================================");
    println!();

    // Основной цикл ядра — ждём прерываний.
    // В будущем здесь будет idle-задача планировщика.
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}

/// Остановить CPU навсегда (без прерываний).
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("cli; hlt"); }
    }
}

/// Обработчик паники — красный экран смерти.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Пытаемся вывести информацию. Если VGA сломан — ну ладно, хотя бы halt.
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);
    println!();
    println!("System halted. Please reboot.");

    halt();
}
