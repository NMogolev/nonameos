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
//   drivers   — драйверная модель (device, driver, PCI bus, Linux shim)
//   vfs       — виртуальная файловая система (ramfs, path lookup)
//   scheduler — планировщик потоков (round-robin, context switch)
//   syscall   — интерфейс системных вызовов (syscall/sysret)
//   ktest     — встроенный тестовый фреймворк
//   userspace — создание user-mode процессов (Ring 3, iretq)
//   loader    — PE загрузчик (парсинг, маппинг секций, imports)
//   win32     — слой совместимости Windows (типы, PE формат, NT API)
// =============================================================================

#![no_std]
#![no_main]

mod vga;
mod gdt;
mod idt;
pub mod pic;
mod serial;
pub mod memory;
pub mod keyboard;
pub mod task;
pub mod ipc;
pub mod drivers;
pub mod vfs;
pub mod scheduler;
mod syscall;
mod ktest;
pub mod multiboot2;
pub mod framebuffer;
pub mod userspace;
pub mod loader;
pub mod win32;

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
    // ЭТАП 2: Framebuffer — графический режим
    // =========================================================================
    // Парсим Multiboot2 info и ищем framebuffer tag.
    // Если GRUB предоставил framebuffer — инициализируем графику.
    let has_framebuffer = unsafe {
        if let Some(fb_info) = multiboot2::find_framebuffer(multiboot_info) {
            framebuffer::init(&fb_info);
            println!("[OK] Framebuffer: {}x{} @ {}bpp (addr: {:#x}, pitch: {})",
                fb_info.width, fb_info.height, fb_info.bpp,
                fb_info.addr, fb_info.pitch);
            true
        } else {
            println!("[WARN] No framebuffer — running in text mode");
            false
        }
    };

    // =========================================================================
    // ЭТАП 3: Отладочный serial порт
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
    // ЭТАП 6: VFS — виртуальная файловая система
    // =========================================================================
    // Инициализируем VFS с ramfs как корневой ФС.
    // Создаются базовые директории: /dev, /sys, /proc, /mnt, /tmp, /drives
    vfs::init();
    println!("[OK] VFS initialized (ramfs root)");

    // =========================================================================
    // ЭТАП 7: PCI Bus — сканирование устройств
    // =========================================================================
    // Перебираем PCI шину, регистрируем найденные устройства в device manager.
    let pci_count = drivers::bus::pci::scan();
    println!("[OK] PCI bus: {} device(s) found", pci_count);

    // Выводим найденные PCI устройства
    for i in 0..pci_count {
        if let Some(dev) = drivers::bus::pci::get_pci_device(i) {
            println!("     {:02x}:{:02x}.{} [{:02x}{:02x}] {:04x}:{:04x}",
                dev.bus, dev.device, dev.function,
                dev.class_code, dev.subclass,
                dev.vendor_id, dev.device_id);
        }
    }

    println!("     Devices registered: {}", drivers::device_count());

    // =========================================================================
    // ЭТАП 8: Scheduler — планировщик потоков
    // =========================================================================
    scheduler::init();
    println!("[OK] Scheduler initialized (idle thread)");

    // =========================================================================
    // ЭТАП 9: Syscall — системные вызовы
    // =========================================================================
    syscall::init();
    println!("[OK] Syscall interface initialized (LSTAR set)");

    // =========================================================================
    // ЭТАП 10: Kernel Worker Threads — многопоточность
    // =========================================================================
    // Спавним рабочие потоки ядра. Они начнут выполняться когда
    // включатся прерывания и таймер вызовет scheduler::schedule().

    // Поток 1: Health Monitor — периодическая проверка подсистем
    let _t1 = scheduler::spawn_kernel_thread_with_priority(
        kthread_health_monitor,
        "health_mon",
        task::Priority::High,
    );

    // Поток 2: Stats Collector — сбор статистики ядра
    let _t2 = scheduler::spawn_kernel_thread_with_priority(
        kthread_stats,
        "stats",
        task::Priority::Low,
    );

    // Поток 3: GC / Reaper — сборщик мёртвых потоков и ресурсов
    let _t3 = scheduler::spawn_kernel_thread_with_priority(
        kthread_reaper,
        "reaper",
        task::Priority::Low,
    );

    println!("[OK] Kernel threads spawned:");
    println!("     Processes: {}  Threads: {}",
        scheduler::process_count(), scheduler::thread_count());

    // =========================================================================
    // ЭТАП 11: Клавиатура + прерывания (включаем многопоточность)
    // =========================================================================
    pic::unmask(1);   // IRQ 1 = PS/2 Keyboard
    pic::unmask(0);   // IRQ 0 = Timer → scheduler (preemptive multitasking)
    idt::enable_interrupts();
    println!("[OK] Interrupts enabled — multithreading ACTIVE");

    // =========================================================================
    // ЭТАП 12: Boot Diagnostics & Recovery
    // =========================================================================
    let test_results = ktest::run_all();

    // =========================================================================
    // ЭТАП 13: Thread List (ps)
    // =========================================================================
    println!("  Active kernel threads:");
    scheduler::list_threads();
    println!();

    // =========================================================================
    // ЭТАП 14: Готовность (зависит от boot mode)
    // =========================================================================
    match test_results.boot_mode {
        ktest::BootMode::Normal => {
            println!("==========================================");
            println!("  NoNameOS kernel initialized.");
            println!("  Subsystems:  memory, vfs, drivers, ipc");
            println!("               scheduler, syscall");
            println!("  Threads: {} active", scheduler::thread_count());
            println!("  Tests: {}/{} passed ({} recovered)",
                test_results.passed, test_results.total, test_results.recovered);
            println!("  Type something — keyboard is active!");
            println!("==========================================");
        }
        ktest::BootMode::Safe => {
            println!("==========================================");
            println!("  NoNameOS — SAFE MODE");
            println!("  Some subsystems are degraded.");
            println!("  Threads: {} active", scheduler::thread_count());
            println!("  Tests: {}/{} passed, {} failed",
                test_results.passed, test_results.total, test_results.failed);
            println!("==========================================");
        }
        ktest::BootMode::Minimal => {
            println!("==========================================");
            println!("  NoNameOS — MINIMAL MODE");
            println!("  Critical subsystem failure detected.");
            println!("  Tests: {}/{} passed, {} failed",
                test_results.passed, test_results.total, test_results.failed);
            println!("==========================================");
        }
    }
    println!();

    // =========================================================================
    // ЭТАП 15: Framebuffer Graphics Demo
    // =========================================================================
    if has_framebuffer {
        let w = framebuffer::width();
        let h = framebuffer::height();

        // Фон — вертикальный градиент (тёмно-синий → чёрный)
        framebuffer::fill_gradient_v(0, 0, w, h, framebuffer::DARK_BLUE, framebuffer::BLACK);

        // Заголовок
        let title = "NoNameOS v0.1.0";
        let title_x = (w - title.len() as u32 * framebuffer::CHAR_WIDTH) / 2;
        framebuffer::draw_string(title_x, 30, title,
            framebuffer::WHITE, framebuffer::DARK_BLUE);

        // Подзаголовок
        let subtitle = "Microkernel OS  |  Rust  |  x86_64";
        let sub_x = (w - subtitle.len() as u32 * framebuffer::CHAR_WIDTH) / 2;
        framebuffer::draw_string(sub_x, 54, subtitle,
            framebuffer::ACCENT, framebuffer::DARK_BLUE);

        // Цветовая палитра
        let colors = [
            (framebuffer::RED,     "RED"),
            (framebuffer::GREEN,   "GREEN"),
            (framebuffer::BLUE,    "BLUE"),
            (framebuffer::CYAN,    "CYAN"),
            (framebuffer::YELLOW,  "YELLOW"),
            (framebuffer::MAGENTA, "MAGENTA"),
            (framebuffer::ORANGE,  "ORANGE"),
            (framebuffer::WHITE,   "WHITE"),
        ];

        let box_w = 100;
        let box_h = 60;
        let gap = 16;
        let total_w = colors.len() as u32 * (box_w + gap) - gap;
        let start_x = (w - total_w) / 2;
        let start_y = 100;

        for (i, (color, name)) in colors.iter().enumerate() {
            let bx = start_x + i as u32 * (box_w + gap);
            framebuffer::fill_rect(bx, start_y, box_w, box_h, *color);
            framebuffer::draw_rect(bx, start_y, box_w, box_h, framebuffer::WHITE, 1);
            let name_x = bx + (box_w - name.len() as u32 * framebuffer::CHAR_WIDTH) / 2;
            framebuffer::draw_string_transparent(name_x, start_y + box_h + 4, name,
                framebuffer::LIGHT_GRAY);
        }

        // Горизонтальный градиент
        let grad_y = start_y + box_h + 40;
        framebuffer::fill_gradient_h(start_x, grad_y, total_w, 30,
            framebuffer::BLUE, framebuffer::RED);
        framebuffer::draw_rect(start_x, grad_y, total_w, 30, framebuffer::WHITE, 1);

        // Информационный блок
        let info_y = grad_y + 50;
        let info_x = start_x;
        framebuffer::draw_string_transparent(info_x, info_y,
            "Framebuffer graphics operational.", framebuffer::FG_LIGHT);
        framebuffer::draw_string_transparent(info_x, info_y + 20,
            "Resolution:       Subsystems: OK", framebuffer::OVERLAY);

        // Рисуем разрешение
        let mut res_buf = [0u8; 20];
        let res_str = format_resolution(w, h, &mut res_buf);
        framebuffer::draw_string_transparent(info_x + 12 * framebuffer::CHAR_WIDTH,
            info_y + 20, res_str, framebuffer::ACCENT);

        println!("[OK] Framebuffer demo rendered ({}x{})", w, h);
    }

    // =========================================================================
    // ЭТАП 16: User-Space — демонстрация загрузки и перехода в Ring 3
    // =========================================================================
    //
    // Загружаем встроенный демо-бинарник в user-space:
    //   1. Создаём новое адресное пространство (PML4)
    //   2. Маппим код по адресу 0x400000 (USER_IMAGE_BASE)
    //   3. Выделяем user stack
    //   4. Переходим в Ring 3 через iretq
    //
    // Демо-код делает: sys_write("Hello from userspace!\n") → sys_exit(0)
    //
    // ВАЖНО: jump_to_usermode() НЕ ВОЗВРАЩАЕТСЯ.
    // Для ядра это аналог exec() — текущий поток превращается в user-поток.
    // В реальной ОС мы бы создали отдельный поток через scheduler,
    // но для демонстрации переходим из idle потока.

    println!("[BOOT] Loading user-space demo binary...");

    match loader::load_raw_binary(userspace::DEMO_USER_CODE, "demo.exe") {
        Ok(image) => {
            println!("[BOOT] User process ready:");
            println!("       Entry: 0x{:X}", image.entry_point);
            println!("       Stack: 0x{:X}", image.user_stack_top);
            println!("       CR3:   0x{:X}", image.cr3);
            println!("[BOOT] Jumping to Ring 3...");
            println!();

            // Переход в user-space — не возвращается!
            userspace::jump_to_usermode(
                image.entry_point,
                image.user_stack_top,
                image.cr3,
            );
        }
        Err(_e) => {
            println!("[WARN] Failed to load user-space demo. Staying in kernel mode.");
        }
    }

    // Основной цикл — idle задача планировщика.
    // По IRQ 0 (таймер) вызывается scheduler::timer_tick().
    // Планировщик переключает контекст на Ready-потоки.
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}

// =============================================================================
// Kernel Worker Threads
// =============================================================================
//
// Потоки ядра, которые выполняются параллельно с idle и друг с другом.
// Планировщик переключает их по таймеру (preemptive) и при sleep (cooperative).

/// kthread: Health Monitor
///
/// Периодически проверяет здоровье подсистем (memory, VFS, scheduler).
/// Если обнаружена деградация — логирует и пытается восстановить.
/// Интервал: ~5 секунд (~91 тик при PIT 18.2 Hz).
fn kthread_health_monitor() -> ! {
    loop {
        scheduler::sleep_ticks(91); // ~5 сек
        let problems = ktest::runtime_health_check();
        if problems > 0 {
            println!("[health_mon] {} subsystem(s) degraded!", problems);
            ktest::print_health_report();
        }
    }
}

/// kthread: Stats Collector
///
/// Собирает статистику ядра: uptime, memory usage, thread count.
/// Записывает в VFS файл /tmp/kstats для диагностики.
/// Интервал: ~10 секунд (~182 тика).
fn kthread_stats() -> ! {
    loop {
        scheduler::sleep_ticks(182); // ~10 сек

        let uptime_ticks = scheduler::ticks();
        let uptime_secs = uptime_ticks / 18; // ~18.2 Hz PIT
        let free_kb = memory::phys::free_memory_kb();
        let total_kb = memory::phys::total_memory_kb();
        let threads = scheduler::thread_count();
        let processes = scheduler::process_count();

        // Пишем статистику в /tmp/kstats (перезаписываем каждый раз)
        let fd = vfs::open(b"/tmp/kstats", 0);
        if let Some(fd) = fd {
            vfs::seek(fd, 0);
            // Формат простой: текстовые строки
            let _ = vfs::write(fd, b"uptime_s:");
            write_usize_to_vfs(fd, uptime_secs as usize);
            let _ = vfs::write(fd, b" mem:");
            write_usize_to_vfs(fd, free_kb);
            let _ = vfs::write(fd, b"/");
            write_usize_to_vfs(fd, total_kb);
            let _ = vfs::write(fd, b"KB thr:");
            write_usize_to_vfs(fd, threads);
            let _ = vfs::write(fd, b" proc:");
            write_usize_to_vfs(fd, processes);
            vfs::close(fd);
        } else {
            // Создаём файл если не существует
            let _ = vfs::create(b"/tmp", b"kstats", vfs::InodeType::File);
        }
    }
}

/// kthread: Reaper (сборщик мёртвых потоков)
///
/// Периодически сканирует таблицу потоков, находит Dead-потоки,
/// освобождает их стеки и слоты. Аналог kthreadd/reaper в Linux.
/// Интервал: ~2 секунды (~36 тиков).
fn kthread_reaper() -> ! {
    loop {
        scheduler::sleep_ticks(36); // ~2 сек
        // В будущем: освобождение стеков Dead-потоков,
        // уменьшение thread_count в процессе, очистка слотов.
        // Пока — noop, заготовка для GC.
    }
}

/// Вспомогательная функция: записать usize в VFS как ASCII строку.
fn write_usize_to_vfs(fd: usize, val: usize) {
    let mut buf = [0u8; 20];
    let mut n = val;
    let mut i = 0;
    if n == 0 {
        buf[0] = b'0';
        i = 1;
    } else {
        while n > 0 && i < 20 {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Переворачиваем
        let mut left = 0;
        let mut right = i - 1;
        while left < right {
            let tmp = buf[left];
            buf[left] = buf[right];
            buf[right] = tmp;
            left += 1;
            right -= 1;
        }
    }
    let _ = vfs::write(fd, &buf[..i]);
}

/// Форматировать разрешение "WxH" в буфер, вернуть &str.
fn format_resolution<'a>(w: u32, h: u32, buf: &'a mut [u8; 20]) -> &'a str {
    let mut pos = 0;

    // Записываем w
    pos = write_u32_to_buf(w, buf, pos);

    // 'x'
    if pos < 20 { buf[pos] = b'x'; pos += 1; }

    // Записываем h
    pos = write_u32_to_buf(h, buf, pos);

    core::str::from_utf8(&buf[..pos]).unwrap_or("?x?")
}

/// Записать u32 в ASCII буфер, вернуть новую позицию.
fn write_u32_to_buf(val: u32, buf: &mut [u8; 20], start: usize) -> usize {
    if val == 0 {
        if start < 20 { buf[start] = b'0'; }
        return start + 1;
    }

    let mut digits = [0u8; 10];
    let mut n = val;
    let mut count = 0;
    while n > 0 {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }

    let mut pos = start;
    for i in (0..count).rev() {
        if pos < 20 { buf[pos] = digits[i]; pos += 1; }
    }
    pos
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
