// =============================================================================
// NoNameOS — Linux Kernel API Shim
// =============================================================================
//
// Цель: предоставить Linux-драйверам привычные API функции,
// реализованные поверх нашего микроядра.
//
// Когда мы портируем Linux-драйвер (например, GPU), он вызывает:
//   kmalloc(), kfree(), ioremap(), printk(), spin_lock(), request_irq()...
//
// Вместо того чтобы переписывать драйвер, мы реализуем эти функции
// как обёртки над примитивами. Это и есть "shim".
//
// Стратегия реализации:
//
//   Этап 1 (сейчас):
//     - kmalloc / kfree / kzalloc     → наш физический аллокатор (page-level)
//     - ioremap / iounmap             → identity mapping (первый 1 GB уже замаплен)
//     - printk                        → наш println!
//     - spin_lock / spin_unlock       → spin::Mutex
//     - udelay / mdelay               → busy-wait
//     - request_irq / free_irq        → наш IDT
//
//   Этап 2 (будущее):
//     - vmalloc / vfree               → виртуальный аллокатор
//     - dma_alloc_coherent            → DMA-совместимые буферы
//     - workqueue                     → отложенная обработка
//     - wait_event / wake_up          → блокировка потоков
//     - struct pci_driver              → интеграция с нашим PCI
//
//   Этап 3 (для Mesa/GPU):
//     - struct drm_device             → DRM подсистема
//     - gem / ttm                     → управление видеопамятью
//     - fb_ops                        → framebuffer
//
// Нейминг: мы сохраняем Linux-имена функций (snake_case, C-style)
// чтобы портированный код требовал минимум изменений.
//
// Аналоги:
//   - FreeBSD linuxkpi (sys/compat/linuxkpi/) — тот же подход
//   - Rust-for-Linux (drivers/rust/) — Rust обёртки над Linux API
// =============================================================================

use crate::memory::{phys, PAGE_SIZE, align_up};

// =============================================================================
// Память: kmalloc / kfree / kzalloc
// =============================================================================
//
// В Linux kmalloc — основной аллокатор ядра (SLAB/SLUB поверх buddy).
// У нас пока page-level аллокатор, поэтому kmalloc выделяет целые страницы.
// Это расточительно для мелких объектов, но работает.
//
// GFP flags (Get Free Pages):
//   GFP_KERNEL  = может спать (для обычного контекста)
//   GFP_ATOMIC  = не может спать (для IRQ обработчиков)
//   GFP_DMA     = память в первых 16 МиБ (для legacy ISA DMA)
//   GFP_DMA32   = память в первых 4 ГиБ
//
// Пока мы игнорируем флаги (наш аллокатор один).

pub type GfpFlags = u32;
pub const GFP_KERNEL: GfpFlags = 0x0;
pub const GFP_ATOMIC: GfpFlags = 0x1;
pub const GFP_DMA: GfpFlags    = 0x2;
pub const GFP_DMA32: GfpFlags  = 0x4;
pub const GFP_ZERO: GfpFlags   = 0x8;

/// Выделить память в ядре.
///
/// `size` — размер в байтах.
/// `flags` — GFP флаги (пока игнорируются).
///
/// Возвращает указатель на начало выделенной памяти,
/// или null если памяти нет.
///
/// Сейчас выделяет целые страницы (4 KiB гранулярность).
/// TODO: slab allocator для мелких объектов.
pub fn kmalloc(size: usize, flags: GfpFlags) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }

    let pages_needed = align_up(size, PAGE_SIZE) / PAGE_SIZE;

    // Пока выделяем только одну страницу.
    // Для multi-page: нужен buddy allocator.
    if pages_needed > 1 {
        // Временное решение: выделяем pages_needed страниц подряд
        // (это работает только если аллокатор выдаёт последовательные фреймы,
        //  что не гарантировано. TODO: buddy allocator)
        let first = phys::alloc_frame();
        match first {
            Some(addr) => {
                // Выделяем остальные
                for _ in 1..pages_needed {
                    if phys::alloc_frame().is_none() {
                        // Не хватило — утечка предыдущих. TODO: откат.
                        return core::ptr::null_mut();
                    }
                }
                let ptr = addr as *mut u8;
                if flags & GFP_ZERO != 0 {
                    unsafe { core::ptr::write_bytes(ptr, 0, pages_needed * PAGE_SIZE); }
                }
                ptr
            }
            None => core::ptr::null_mut(),
        }
    } else {
        match phys::alloc_frame() {
            Some(addr) => {
                let ptr = addr as *mut u8;
                if flags & GFP_ZERO != 0 {
                    unsafe { core::ptr::write_bytes(ptr, 0, PAGE_SIZE); }
                }
                ptr
            }
            None => core::ptr::null_mut(),
        }
    }
}

/// Выделить обнулённую память.
pub fn kzalloc(size: usize, flags: GfpFlags) -> *mut u8 {
    kmalloc(size, flags | GFP_ZERO)
}

/// Освободить память, выделенную через kmalloc.
///
/// Сейчас освобождаем только первую страницу.
/// TODO: отслеживать размер аллокации.
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let addr = ptr as usize;
    // Выравниваем на страницу
    let frame_addr = addr & !(PAGE_SIZE - 1);
    phys::free_frame(frame_addr);
}

// =============================================================================
// I/O Memory: ioremap / iounmap
// =============================================================================
//
// В Linux ioremap() маппит физические адреса устройств (MMIO)
// в виртуальное адресное пространство ядра.
//
// У нас первый 1 ГиБ уже identity-mapped (boot.asm),
// поэтому для устройств в этом диапазоне ioremap — тождественное отображение.
// Для адресов выше 1 ГиБ (PCIe BARs) нужно будет создавать маппинги.

/// Замапить физический адрес устройства для доступа из ядра.
///
/// Пока: identity mapping (физ. адрес = вирт. адрес).
/// TODO: для адресов > 1 GB создавать маппинг через paging.
pub fn ioremap(phys_addr: u64, size: u64) -> *mut u8 {
    // Адреса в первом 1 GB уже замаплены через identity mapping
    // Для PCI BAR обычно < 4 GB, но может быть и выше.
    // TODO: создать маппинг для высоких адресов
    phys_addr as *mut u8
}

/// Отменить маппинг MMIO региона.
pub fn iounmap(_virt_addr: *mut u8, _size: u64) {
    // С identity mapping ничего делать не нужно.
    // TODO: при реальном маппинге — unmap страницы.
}

// =============================================================================
// Чтение/запись MMIO регистров
// =============================================================================
//
// Для работы с устройствами через MMIO нужны volatile операции.
// В Linux: readl(), writel(), readq(), writeq().

/// Прочитать 32-bit регистр устройства (MMIO).
#[inline(always)]
pub unsafe fn readl(addr: *const u32) -> u32 {
    core::ptr::read_volatile(addr)
}

/// Записать 32-bit регистр устройства (MMIO).
#[inline(always)]
pub unsafe fn writel(value: u32, addr: *mut u32) {
    core::ptr::write_volatile(addr, value);
}

/// Прочитать 64-bit регистр.
#[inline(always)]
pub unsafe fn readq(addr: *const u64) -> u64 {
    core::ptr::read_volatile(addr)
}

/// Записать 64-bit регистр.
#[inline(always)]
pub unsafe fn writeq(value: u64, addr: *mut u64) {
    core::ptr::write_volatile(addr, value);
}

/// Прочитать 16-bit регистр.
#[inline(always)]
pub unsafe fn readw(addr: *const u16) -> u16 {
    core::ptr::read_volatile(addr)
}

/// Записать 16-bit регистр.
#[inline(always)]
pub unsafe fn writew(value: u16, addr: *mut u16) {
    core::ptr::write_volatile(addr, value);
}

/// Прочитать 8-bit регистр.
#[inline(always)]
pub unsafe fn readb(addr: *const u8) -> u8 {
    core::ptr::read_volatile(addr)
}

/// Записать 8-bit регистр.
#[inline(always)]
pub unsafe fn writeb(value: u8, addr: *mut u8) {
    core::ptr::write_volatile(addr, value);
}

// =============================================================================
// Задержки: udelay / mdelay
// =============================================================================
//
// Многие драйверы ждут определённое время после записи в регистр.
// В Linux udelay() использует calibrated busy-loop (BogoMIPS).
// У нас пока грубый busy-wait.

/// Задержка в микросекундах (busy-wait, приблизительная).
///
/// Калибровка: ~1000 итераций ≈ 1 мкс на ~1 GHz CPU.
/// Это очень приблизительно. TODO: калибровка через PIT/TSC.
pub fn udelay(us: u64) {
    let iterations = us * 1000;
    for _ in 0..iterations {
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
}

/// Задержка в миллисекундах.
pub fn mdelay(ms: u64) {
    udelay(ms * 1000);
}

// =============================================================================
// Spinlock
// =============================================================================
//
// В Linux: spin_lock_irqsave / spin_unlock_irqrestore.
// Мы используем простой атомарный спинлок.
// В будущем нужно добавить disable/enable interrupts.

use core::sync::atomic::{AtomicBool, Ordering};

/// Простой спинлок (аналог spinlock_t в Linux).
pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
        }
    }

    /// Захватить лок (busy-wait до успеха).
    pub fn lock(&self) {
        while self.locked.compare_exchange_weak(
            false, true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_err() {
            // Hint для CPU: мы в spin-loop
            core::hint::spin_loop();
        }
    }

    /// Отпустить лок.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Попробовать захватить без ожидания.
    pub fn try_lock(&self) -> bool {
        self.locked.compare_exchange(
            false, true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_ok()
    }
}

// =============================================================================
// Atomic операции
// =============================================================================
//
// Linux: atomic_t, atomic_read(), atomic_set(), atomic_add(), atomic_inc()...

use core::sync::atomic::AtomicI32;

/// Аналог atomic_t из Linux.
pub struct AtomicInt {
    value: AtomicI32,
}

impl AtomicInt {
    pub const fn new(val: i32) -> Self {
        AtomicInt { value: AtomicI32::new(val) }
    }

    pub fn read(&self) -> i32 {
        self.value.load(Ordering::SeqCst)
    }

    pub fn set(&self, val: i32) {
        self.value.store(val, Ordering::SeqCst);
    }

    pub fn add(&self, val: i32) -> i32 {
        self.value.fetch_add(val, Ordering::SeqCst)
    }

    pub fn sub(&self, val: i32) -> i32 {
        self.value.fetch_sub(val, Ordering::SeqCst)
    }

    pub fn inc(&self) -> i32 {
        self.add(1)
    }

    pub fn dec(&self) -> i32 {
        self.sub(1)
    }
}

// =============================================================================
// I/O порты
// =============================================================================
//
// Linux: inb(), outb(), inw(), outw(), inl(), outl()

/// Записать байт в I/O порт.
#[inline(always)]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Прочитать байт из I/O порта.
#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Записать 16 бит в I/O порт.
#[inline(always)]
pub unsafe fn outw(port: u16, value: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Прочитать 16 бит из I/O порта.
#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    core::arch::asm!(
        "in ax, dx",
        in("dx") port,
        out("ax") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Записать 32 бита в I/O порт.
#[inline(always)]
pub unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Прочитать 32 бита из I/O порта.
#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}

// =============================================================================
// Логирование: printk
// =============================================================================
//
// В Linux printk — основная функция логирования с уровнями:
//   KERN_EMERG, KERN_ALERT, KERN_CRIT, KERN_ERR,
//   KERN_WARNING, KERN_NOTICE, KERN_INFO, KERN_DEBUG
//
// У нас пока просто уровни + вывод через наш println!.

/// Уровни логирования (как в Linux).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum LogLevel {
    Emergency = 0,
    Alert     = 1,
    Critical  = 2,
    Error     = 3,
    Warning   = 4,
    Notice    = 5,
    Info      = 6,
    Debug     = 7,
}

/// Текущий уровень логирования (сообщения ниже этого уровня отбрасываются).
static mut CURRENT_LOG_LEVEL: LogLevel = LogLevel::Info;

/// Установить уровень логирования.
pub fn set_log_level(level: LogLevel) {
    unsafe { CURRENT_LOG_LEVEL = level; }
}

/// Макрос для логирования из драйверов.
/// Использование: log!(LogLevel::Info, "PCI device found: {:04x}:{:04x}", vendor, device);
#[macro_export]
macro_rules! driver_log {
    ($level:expr, $($arg:tt)*) => {{
        use $crate::drivers::linux_shim::LogLevel;
        let lvl: LogLevel = $level;
        let current = unsafe { $crate::drivers::linux_shim::CURRENT_LOG_LEVEL };
        if (lvl as u8) <= (current as u8) {
            let prefix = match lvl {
                LogLevel::Emergency => "[EMERG]",
                LogLevel::Alert     => "[ALERT]",
                LogLevel::Critical  => "[CRIT] ",
                LogLevel::Error     => "[ERR]  ",
                LogLevel::Warning   => "[WARN] ",
                LogLevel::Notice    => "[NOTE] ",
                LogLevel::Info      => "[INFO] ",
                LogLevel::Debug     => "[DBG]  ",
            };
            $crate::println!("{} {}", prefix, format_args!($($arg)*));
        }
    }};
}

// =============================================================================
// Errno — коды ошибок Linux
// =============================================================================
//
// Linux-драйверы возвращают отрицательные errno при ошибке.

pub const ENOMEM: i32    = -12;
pub const ENODEV: i32    = -19;
pub const EINVAL: i32    = -22;
pub const ENOSYS: i32    = -38;
pub const ENOENT: i32    = -2;
pub const EIO: i32       = -5;
pub const EBUSY: i32     = -16;
pub const EEXIST: i32    = -17;
pub const EPERM: i32     = -1;
pub const EAGAIN: i32    = -11;
pub const EFAULT: i32    = -14;
pub const ETIMEDOUT: i32 = -110;
