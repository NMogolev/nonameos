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
//   Этап 1:
//     - kmalloc / kfree / kzalloc     → физический аллокатор
//     - ioremap / iounmap             → identity mapping
//     - printk                        → println!
//     - spin_lock / spin_unlock       → spin::Mutex
//     - udelay / mdelay               → busy-wait
//     - request_irq / free_irq        → IDT
//
//   Этап 2:
//     - vmalloc / vfree               → виртуальный аллокатор
//     - dma_alloc_coherent            → DMA-совместимые буферы
//     - workqueue                     → отложенная обработка
//     - wait_event / wake_up          → блокировка потоков
//     - struct pci_driver              → интеграция с нашим PCI
//
//   Этап 3:
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
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

// =============================================================================
// Аллокационная таблица: отслеживание размера kmalloc аллокаций
// =============================================================================
//
// Без heap-аллокатора мы не можем использовать HashMap.
// Простой массив (addr, page_count) — достаточно для MVP.
// Максимум MAX_ALLOC_ENTRIES одновременных аллокаций.

const MAX_ALLOC_ENTRIES: usize = 256;

struct AllocEntry {
    addr: usize,      // физический адрес (0 = свободный слот)
    page_count: usize, // сколько страниц выделено
}

static ALLOC_TABLE_LOCK: AtomicBool = AtomicBool::new(false);
static mut ALLOC_TABLE: [AllocEntry; MAX_ALLOC_ENTRIES] = {
    const EMPTY: AllocEntry = AllocEntry { addr: 0, page_count: 0 };
    [EMPTY; MAX_ALLOC_ENTRIES]
};

/// Записать аллокацию в таблицу.
fn alloc_table_insert(addr: usize, pages: usize) {
    while ALLOC_TABLE_LOCK.compare_exchange_weak(
        false, true, Ordering::Acquire, Ordering::Relaxed
    ).is_err() {
        core::hint::spin_loop();
    }
    unsafe {
        let table = core::ptr::addr_of_mut!(ALLOC_TABLE);
        for i in 0..MAX_ALLOC_ENTRIES {
            let entry = &mut (*table)[i];
            if entry.addr == 0 {
                entry.addr = addr;
                entry.page_count = pages;
                break;
            }
        }
    }
    ALLOC_TABLE_LOCK.store(false, Ordering::Release);
}

/// Найти и удалить аллокацию, вернуть количество страниц.
fn alloc_table_remove(addr: usize) -> usize {
    while ALLOC_TABLE_LOCK.compare_exchange_weak(
        false, true, Ordering::Acquire, Ordering::Relaxed
    ).is_err() {
        core::hint::spin_loop();
    }
    let mut pages = 0;
    unsafe {
        let table = core::ptr::addr_of_mut!(ALLOC_TABLE);
        for i in 0..MAX_ALLOC_ENTRIES {
            let entry = &mut (*table)[i];
            if entry.addr == addr {
                pages = entry.page_count;
                entry.addr = 0;
                entry.page_count = 0;
                break;
            }
        }
    }
    ALLOC_TABLE_LOCK.store(false, Ordering::Release);
    pages
}

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
/// `flags` — GFP флаги (GFP_ZERO обнуляет; остальные пока игнорируются).
///
/// Возвращает указатель на начало выделенной памяти,
/// или null если памяти нет.
///
/// Сейчас выделяет целые страницы (4 KiB гранулярность).
/// Multi-page аллокации: при неудаче все уже выделенные страницы
/// корректно откатываются (нет утечки).
///
/// ОГРАНИЧЕНИЕ: multi-page аллокации предполагают, что аллокатор
/// выдаёт последовательные фреймы. Это НЕ гарантировано.
/// TODO: buddy allocator для настоящих contiguous аллокаций.
pub fn kmalloc(size: usize, flags: GfpFlags) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }

    let pages_needed = align_up(size, PAGE_SIZE) / PAGE_SIZE;

    // Массив для отката: храним адреса выделенных фреймов.
    // Ограничение: максимум 64 страниц (256 KiB) за один kmalloc.
    const MAX_PAGES_PER_ALLOC: usize = 64;
    if pages_needed > MAX_PAGES_PER_ALLOC {
        return core::ptr::null_mut();
    }

    let mut frames: [usize; MAX_PAGES_PER_ALLOC] = [0; MAX_PAGES_PER_ALLOC];
    let mut allocated = 0usize;

    for i in 0..pages_needed {
        match phys::alloc_frame() {
            Some(addr) => {
                frames[i] = addr;
                allocated += 1;
            }
            None => {
                // Откат: освобождаем все уже выделенные фреймы
                for j in 0..allocated {
                    phys::free_frame(frames[j]);
                }
                return core::ptr::null_mut();
            }
        }
    }

    let ptr = frames[0] as *mut u8;

    // Записываем в таблицу аллокаций для корректного kfree
    alloc_table_insert(frames[0], pages_needed);

    if flags & GFP_ZERO != 0 {
        unsafe { core::ptr::write_bytes(ptr, 0, pages_needed * PAGE_SIZE); }
    }

    ptr
}

/// Выделить обнулённую память.
pub fn kzalloc(size: usize, flags: GfpFlags) -> *mut u8 {
    kmalloc(size, flags | GFP_ZERO)
}

/// Освободить память, выделенную через kmalloc.
///
/// Использует таблицу аллокаций для определения количества страниц.
/// Если аллокация не найдена — освобождает одну страницу (fallback).
pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let addr = ptr as usize;
    let frame_addr = addr & !(PAGE_SIZE - 1);

    let pages = alloc_table_remove(frame_addr);
    if pages == 0 {
        // Не нашли в таблице — освобождаем одну страницу (legacy fallback)
        phys::free_frame(frame_addr);
    } else {
        for i in 0..pages {
            phys::free_frame(frame_addr + i * PAGE_SIZE);
        }
    }
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

/// Максимальный физический адрес, покрытый identity mapping (boot.asm).
const IDENTITY_MAP_LIMIT: u64 = 0x4000_0000; // 1 GiB

/// Замапить физический адрес устройства для доступа из ядра.
///
/// Поддерживает только адреса < 1 GiB (identity mapping из boot.asm).
/// Для адресов >= 1 GiB возвращает None.
///
/// Возвращает `Option<*mut u8>` вместо голого указателя,
/// чтобы драйвер мог корректно обработать ошибку.
///
/// TODO: для высоких адресов создавать маппинг через paging модуль.
pub fn ioremap(phys_addr: u64, _size: u64) -> Option<*mut u8> {
    if phys_addr < IDENTITY_MAP_LIMIT {
        Some(phys_addr as *mut u8)
    } else {
        // Адрес выше identity map — нужен page table mapping.
        // Пока не реализовано.
        None
    }
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

/// Частота CPU в MHz для TSC-калибровки.
/// Грубая оценка — будет уточнена при инициализации через PIT.
/// Для QEMU/KVM обычно ~2000-3000 MHz.
static CPU_MHZ: AtomicU32 = AtomicU32::new(1000); // fallback: 1 GHz

/// Установить частоту CPU (вызывать после калибровки через PIT).
pub fn set_cpu_mhz(mhz: u32) {
    CPU_MHZ.store(mhz, Ordering::Relaxed);
}

/// Задержка в микросекундах.
///
/// Использует TSC (RDTSC) для точного timing.
/// Точность зависит от правильной калибровки CPU_MHZ.
/// По умолчанию предполагает 1 GHz; вызовите set_cpu_mhz() для коррекции.
pub fn udelay(us: u64) {
    let mhz = CPU_MHZ.load(Ordering::Relaxed) as u64;
    let target_cycles = us * mhz;

    let start_lo: u32;
    let start_hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") start_lo, out("edx") start_hi,
            options(nomem, nostack));
    }
    let start = ((start_hi as u64) << 32) | (start_lo as u64);

    loop {
        let now_lo: u32;
        let now_hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") now_lo, out("edx") now_hi,
                options(nomem, nostack));
        }
        let now = ((now_hi as u64) << 32) | (now_lo as u64);
        if now.wrapping_sub(start) >= target_cycles {
            break;
        }
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

/// Простой спинлок (аналог spinlock_t в Linux).
///
/// Поддерживает два режима:
///   - `lock()` / `unlock()` — без управления прерываниями
///   - `lock_irqsave()` / `unlock_irqrestore()` — отключает IRQ,
///     предотвращая deadlock при захвате из IRQ-контекста
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
    /// НЕ отключает прерывания. Если лок может быть захвачен из IRQ —
    /// используйте lock_irqsave().
    pub fn lock(&self) {
        while self.locked.compare_exchange_weak(
            false, true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_err() {
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

    /// Захватить лок + отключить прерывания.
    /// Возвращает RFLAGS для последующего восстановления.
    ///
    /// Аналог spin_lock_irqsave() в Linux.
    /// Предотвращает deadlock при захвате из IRQ-обработчика.
    pub fn lock_irqsave(&self) -> u64 {
        let flags = save_flags_and_cli();
        self.lock();
        flags
    }

    /// Отпустить лок + восстановить прерывания.
    ///
    /// Аналог spin_unlock_irqrestore() в Linux.
    pub fn unlock_irqrestore(&self, flags: u64) {
        self.unlock();
        restore_flags(flags);
    }
}

/// Сохранить RFLAGS и отключить прерывания (CLI).
#[inline(always)]
fn save_flags_and_cli() -> u64 {
    let flags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            "cli",
            out(reg) flags,
            options(nomem, preserves_flags)
        );
    }
    flags
}

/// Восстановить RFLAGS (включая бит IF — прерывания).
#[inline(always)]
fn restore_flags(flags: u64) {
    unsafe {
        core::arch::asm!(
            "push {}",
            "popfq",
            in(reg) flags,
            options(nomem)
        );
    }
}

// =============================================================================
// Atomic операции
// =============================================================================
//
// Linux: atomic_t, atomic_read(), atomic_set(), atomic_add(), atomic_inc()...

/// Аналог atomic_t из Linux (signed 32-bit).
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

/// Аналог atomic_t unsigned (unsigned 32-bit).
/// Используется в Linux для счётчиков ссылок, etc.
pub struct AtomicUint {
    value: AtomicU32,
}

impl AtomicUint {
    pub const fn new(val: u32) -> Self {
        AtomicUint { value: AtomicU32::new(val) }
    }

    pub fn read(&self) -> u32 {
        self.value.load(Ordering::SeqCst)
    }

    pub fn set(&self, val: u32) {
        self.value.store(val, Ordering::SeqCst);
    }

    pub fn add(&self, val: u32) -> u32 {
        self.value.fetch_add(val, Ordering::SeqCst)
    }

    pub fn sub(&self, val: u32) -> u32 {
        self.value.fetch_sub(val, Ordering::SeqCst)
    }

    pub fn inc(&self) -> u32 {
        self.add(1)
    }

    pub fn dec(&self) -> u32 {
        self.sub(1)
    }
}

/// Атомарный указатель (аналог atomic_long_t / rcu pointer в Linux).
pub struct AtomicPtr<T> {
    value: AtomicUsize,
    _marker: core::marker::PhantomData<*mut T>,
}

impl<T> AtomicPtr<T> {
    pub fn new(ptr: *mut T) -> Self {
        AtomicPtr {
            value: AtomicUsize::new(ptr as usize),
            _marker: core::marker::PhantomData,
        }
    }

    pub const fn null() -> Self {
        AtomicPtr {
            value: AtomicUsize::new(0),
            _marker: core::marker::PhantomData,
        }
    }

    pub fn load(&self) -> *mut T {
        self.value.load(Ordering::SeqCst) as *mut T
    }

    pub fn store(&self, ptr: *mut T) {
        self.value.store(ptr as usize, Ordering::SeqCst);
    }

    pub fn is_null(&self) -> bool {
        self.value.load(Ordering::SeqCst) == 0
    }
}

// SAFETY: AtomicPtr содержит только атомарный usize
unsafe impl<T> Send for AtomicPtr<T> {}
unsafe impl<T> Sync for AtomicPtr<T> {}

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
