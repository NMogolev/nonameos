// =============================================================================
// NoNameOS — Физический аллокатор памяти (Bitmap Allocator)
// =============================================================================
//
// Что это делает:
//   Управляет физической RAM. Вся память разбита на «фреймы» (frames)
//   по 4 КиБ. Аллокатор знает, какие фреймы свободны, какие заняты.
//
// Как работает bitmap:
//   Каждый бит в массиве байтов отвечает за один фрейм:
//     бит 0 → фрейм 0  (физ. адрес 0x00000 — 0x00FFF)
//     бит 1 → фрейм 1  (физ. адрес 0x01000 — 0x01FFF)
//     бит N → фрейм N  (физ. адрес N*4096 — (N+1)*4096 - 1)
//
//   Значение бита:
//     0 = фрейм свободен, можно выделить
//     1 = фрейм занят (ядром, устройством, или пользовательским процессом)
//
// Для 4 ГиБ RAM нужно: 4 ГиБ / 4 КиБ = 1 048 576 фреймов = 128 КиБ bitmap.
// Это всего ~0.003% от общей памяти. Очень дёшево!
//
// Альтернативы (на будущее):
//   - Buddy Allocator — быстрее для больших блоков, используется в Linux
//   - Slab Allocator  — для объектов фиксированного размера (поверх buddy)
//
// Пока bitmap — простой и понятный старт.
// =============================================================================

use super::{PAGE_SIZE, align_up};

/// Максимум памяти, которую мы поддерживаем — 512 МиБ (для начала).
/// 512 МиБ / 4 КиБ = 131 072 фреймов.
const MAX_FRAMES: usize = 131_072;

/// Bitmap: каждый байт хранит состояние 8 фреймов.
/// 131 072 / 8 = 16 384 байт = 16 КиБ для bitmap.
const BITMAP_SIZE: usize = MAX_FRAMES / 8;

/// Статический bitmap. Живёт в .bss секции ядра (обнулён при загрузке).
/// По умолчанию все биты = 0, значит все фреймы считаются «свободными».
/// При init() мы помечаем занятые области (ядро, BIOS и т.д.)
static mut BITMAP: [u8; BITMAP_SIZE] = [0u8; BITMAP_SIZE];

/// Количество реально доступных фреймов (определяется при init).
static mut TOTAL_FRAMES: usize = 0;

/// Количество свободных фреймов.
static mut FREE_FRAMES: usize = 0;

// ---- Вспомогательные функции для работы с битами ----

/// Пометить фрейм как занятый (бит = 1).
fn set_bit(frame: usize) {
    unsafe {
        let byte = frame / 8;       // какой байт в bitmap
        let bit = frame % 8;        // какой бит в этом байте
        BITMAP[byte] |= 1 << bit;   // установить бит
    }
}

/// Пометить фрейм как свободный (бит = 0).
fn clear_bit(frame: usize) {
    unsafe {
        let byte = frame / 8;
        let bit = frame % 8;
        BITMAP[byte] &= !(1 << bit); // сбросить бит
    }
}

/// Проверить, занят ли фрейм.
fn test_bit(frame: usize) -> bool {
    unsafe {
        let byte = frame / 8;
        let bit = frame % 8;
        (BITMAP[byte] & (1 << bit)) != 0
    }
}

/// Физический адрес → номер фрейма.
/// Пример: 0x5000 → фрейм 5
fn addr_to_frame(addr: usize) -> usize {
    addr / PAGE_SIZE
}

/// Номер фрейма → физический адрес начала фрейма.
/// Пример: фрейм 5 → 0x5000
fn frame_to_addr(frame: usize) -> usize {
    frame * PAGE_SIZE
}

// ---- Публичный API ----

/// Инициализация аллокатора.
///
/// `memory_size` — общий объём RAM в байтах (например, 256 * 1024 * 1024 для 256 МиБ).
/// `kernel_start` / `kernel_end` — диапазон физических адресов ядра.
///
/// Что происходит:
///   1. Считаем количество фреймов
///   2. Помечаем первые 1 МиБ как занятые (BIOS, VGA, legacy)
///   3. Помечаем область ядра как занятую
///   4. Всё остальное — свободно для использования
pub fn init(memory_size: usize, kernel_start: usize, kernel_end: usize) {
    let total = core::cmp::min(memory_size / PAGE_SIZE, MAX_FRAMES);

    unsafe {
        TOTAL_FRAMES = total;
        FREE_FRAMES = total;

        // Шаг 1: Пометить ВСЁ как свободное (bitmap уже в .bss = нули)
        // (ничего делать не нужно, .bss обнулён)

        // Шаг 2: Зарезервировать первый 1 МиБ
        // Там живут:
        //   0x00000 — 0x003FF  IVT (Interrupt Vector Table, legacy BIOS)
        //   0x00400 — 0x004FF  BDA (BIOS Data Area)
        //   0x80000 — 0x9FFFF  EBDA (Extended BIOS Data Area)
        //   0xA0000 — 0xBFFFF  VGA Video Memory
        //   0xC0000 — 0xFFFFF  BIOS ROM, Option ROMs
        let reserved_end = align_up(1024 * 1024, PAGE_SIZE); // 1 МиБ
        let reserved_frames = reserved_end / PAGE_SIZE;       // 256 фреймов
        for i in 0..reserved_frames {
            set_bit(i);
            FREE_FRAMES -= 1;
        }

        // Шаг 3: Зарезервировать область ядра
        let kstart_frame = addr_to_frame(kernel_start);
        let kend_frame = addr_to_frame(align_up(kernel_end, PAGE_SIZE));
        for i in kstart_frame..kend_frame {
            if !test_bit(i) {
                set_bit(i);
                FREE_FRAMES -= 1;
            }
        }
    }
}

/// Выделить один физический фрейм (4 КиБ).
/// Возвращает физический адрес начала фрейма, или None если память кончилась.
///
/// Алгоритм: линейный поиск первого свободного бита.
/// Это O(n) — медленно для большой памяти. В будущем заменим на buddy allocator.
pub fn alloc_frame() -> Option<usize> {
    unsafe {
        for i in 0..TOTAL_FRAMES {
            if !test_bit(i) {
                set_bit(i);
                FREE_FRAMES -= 1;
                return Some(frame_to_addr(i));
            }
        }
    }
    None // Памяти нет!
}

/// Освободить один физический фрейм.
/// `addr` — физический адрес начала фрейма (должен быть выровнен на 4 КиБ).
pub fn free_frame(addr: usize) {
    let frame = addr_to_frame(addr);
    if test_bit(frame) {
        clear_bit(frame);
        unsafe { FREE_FRAMES += 1; }
    }
}

/// Количество свободных фреймов.
pub fn free_count() -> usize {
    unsafe { FREE_FRAMES }
}

/// Общее количество фреймов.
pub fn total_count() -> usize {
    unsafe { TOTAL_FRAMES }
}

/// Свободная память в килобайтах.
pub fn free_memory_kb() -> usize {
    free_count() * (PAGE_SIZE / 1024)
}

/// Общая память в килобайтах.
pub fn total_memory_kb() -> usize {
    total_count() * (PAGE_SIZE / 1024)
}
