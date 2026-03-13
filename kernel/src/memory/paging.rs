// =============================================================================
// NoNameOS — Виртуальная память (Page Tables)
// =============================================================================
//
// Как CPU транслирует виртуальный адрес в физический:
//
//   Виртуальный адрес (64 бита, но используются только 48):
//   ┌────────────┬────────┬────────┬────────┬────────┬──────────────┐
//   │ Sign Ext.  │ PML4   │ PDPT   │ PD     │ PT     │ Offset       │
//   │ бит 63..48 │ 47..39 │ 38..30 │ 29..21 │ 20..12 │ 11..0        │
//   └────────────┴───┬────┴───┬────┴───┬────┴───┬────┴──────────────┘
//                    │        │        │        │
//                    ▼        ▼        ▼        ▼
//                  PML4 →   PDPT →    PD →     PT → Физический адрес
//                 (CR3)
//
// Каждая таблица — массив из 512 записей по 8 байт = 4 КиБ (ровно 1 страница).
//
// Биты записи (Page Table Entry):
//   Бит 0  — Present (P):      1 = запись активна
//   Бит 1  — Writable (W):     1 = можно писать
//   Бит 2  — User (U):         1 = доступна из Ring 3
//   Бит 3  — Write-Through:    кэширование
//   Бит 4  — Cache Disable:    отключить кэш (для MMIO)
//   Бит 5  — Accessed (A):     CPU ставит 1 при обращении
//   Бит 6  — Dirty (D):        CPU ставит 1 при записи (только в PT)
//   Бит 7  — Huge Page (PS):   2 МиБ страница (в PD) / 1 ГиБ (в PDPT)
//   Бит 63 — No Execute (NX):  запрет исполнения (нужен EFER.NXE)
//
// Пример:
//   Хотим замапить виртуальный адрес 0x400000 (4 МиБ) на физический 0x200000:
//   1. PML4[0] → адрес PDPT таблицы
//   2. PDPT[0] → адрес PD таблицы
//   3. PD[2]   → адрес PT таблицы          (0x400000 >> 21 = 2)
//   4. PT[0]   → 0x200000 | Present | Write  (итоговый маппинг)
//
// =============================================================================

use super::phys;
use super::PAGE_SIZE;

/// Флаги записи в таблице страниц.
/// Каждый флаг — один бит. Комбинируются через OR (|).
pub mod flags {
    pub const PRESENT: u64    = 1 << 0;  // Запись активна
    pub const WRITABLE: u64   = 1 << 1;  // Разрешена запись
    pub const USER: u64       = 1 << 2;  // Доступно из Ring 3 (user-space)
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const CACHE_DISABLE: u64 = 1 << 4;
    pub const HUGE_PAGE: u64  = 1 << 7;  // 2 MiB страница (в PD уровне)
    pub const NO_EXECUTE: u64 = 1 << 63; // Запрет исполнения кода
}

/// Одна запись в таблице страниц (любого уровня).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageEntry(u64);

impl PageEntry {
    /// Пустая запись (not present).
    pub const fn empty() -> Self {
        PageEntry(0)
    }

    /// Создать запись: физический адрес + флаги.
    pub const fn new(phys_addr: u64, flags: u64) -> Self {
        // Адрес занимает биты 12..51 (маскируем, чтобы не испортить флаги)
        PageEntry((phys_addr & 0x000F_FFFF_FFFF_F000) | flags)
    }

    /// Запись активна? (бит Present установлен)
    pub fn is_present(&self) -> bool {
        self.0 & flags::PRESENT != 0
    }

    /// Извлечь физический адрес из записи (биты 12..51).
    pub fn address(&self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }

    /// Получить флаги записи (нижние 12 бит + бит 63).
    pub fn flags(&self) -> u64 {
        self.0 & !0x000F_FFFF_FFFF_F000
    }
}

/// Таблица страниц — массив из 512 записей.
/// Размер ровно 4096 байт = 1 страница.
#[repr(align(4096))]
pub struct PageTable {
    pub entries: [PageEntry; 512],
}

impl PageTable {
    /// Создать пустую таблицу (все записи not present).
    pub const fn empty() -> Self {
        PageTable {
            entries: [PageEntry::empty(); 512],
        }
    }
}

// ---- Индексы из виртуального адреса ----
// Виртуальный адрес разбивается на 4 индекса (по 9 бит каждый)

/// Индекс в PML4 (биты 47..39)
fn pml4_index(virt: usize) -> usize {
    (virt >> 39) & 0x1FF
}

/// Индекс в PDPT (биты 38..30)
fn pdpt_index(virt: usize) -> usize {
    (virt >> 30) & 0x1FF
}

/// Индекс в PD (биты 29..21)
fn pd_index(virt: usize) -> usize {
    (virt >> 21) & 0x1FF
}

/// Индекс в PT (биты 20..12)
fn pt_index(virt: usize) -> usize {
    (virt >> 12) & 0x1FF
}

// ---- Публичный API ----

/// Прочитать текущий CR3 — физический адрес PML4 таблицы.
/// CR3 загружается при создании процесса; у каждого процесса свой CR3.
pub fn read_cr3() -> u64 {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    cr3
}

/// Замапить одну 4 КиБ страницу: виртуальный адрес → физический.
///
/// `pml4_addr` — физический адрес PML4 таблицы (из CR3).
/// `virt_addr` — виртуальный адрес для маппинга.
/// `phys_addr` — физический адрес, куда он должен указывать.
/// `user` — true = доступно из user-space (Ring 3).
///
/// Если промежуточные таблицы (PDPT, PD, PT) не существуют,
/// функция выделяет для них новые физические фреймы.
pub fn map_page(pml4_addr: u64, virt_addr: usize, phys_addr: usize, user: bool) {
    let base_flags = flags::PRESENT | flags::WRITABLE
        | if user { flags::USER } else { 0 };

    // Получаем указатель на PML4
    let pml4 = unsafe { &mut *(pml4_addr as *mut PageTable) };

    // Шаг 1: PML4 → PDPT
    let pml4_i = pml4_index(virt_addr);
    if !pml4.entries[pml4_i].is_present() {
        // Таблицы PDPT нет — выделяем новый фрейм и создаём пустую
        let frame = phys::alloc_frame().expect("map_page: out of memory for PDPT");
        zero_frame(frame);
        pml4.entries[pml4_i] = PageEntry::new(frame as u64, base_flags);
    }
    let pdpt = unsafe { &mut *(pml4.entries[pml4_i].address() as *mut PageTable) };

    // Шаг 2: PDPT → PD
    let pdpt_i = pdpt_index(virt_addr);
    if !pdpt.entries[pdpt_i].is_present() {
        let frame = phys::alloc_frame().expect("map_page: out of memory for PD");
        zero_frame(frame);
        pdpt.entries[pdpt_i] = PageEntry::new(frame as u64, base_flags);
    }
    let pd = unsafe { &mut *(pdpt.entries[pdpt_i].address() as *mut PageTable) };

    // Шаг 3: PD → PT
    let pd_i = pd_index(virt_addr);
    if !pd.entries[pd_i].is_present() {
        let frame = phys::alloc_frame().expect("map_page: out of memory for PT");
        zero_frame(frame);
        pd.entries[pd_i] = PageEntry::new(frame as u64, base_flags);
    }
    let pt = unsafe { &mut *(pd.entries[pd_i].address() as *mut PageTable) };

    // Шаг 4: записываем итоговый маппинг в PT
    let pt_i = pt_index(virt_addr);
    pt.entries[pt_i] = PageEntry::new(phys_addr as u64, base_flags);
}

/// Убрать маппинг виртуального адреса (unmap).
/// Не освобождает физический фрейм — это ответственность вызывающего.
pub fn unmap_page(pml4_addr: u64, virt_addr: usize) {
    let pml4 = unsafe { &mut *(pml4_addr as *mut PageTable) };

    let pml4_i = pml4_index(virt_addr);
    if !pml4.entries[pml4_i].is_present() { return; }
    let pdpt = unsafe { &mut *(pml4.entries[pml4_i].address() as *mut PageTable) };

    let pdpt_i = pdpt_index(virt_addr);
    if !pdpt.entries[pdpt_i].is_present() { return; }
    let pd = unsafe { &mut *(pdpt.entries[pdpt_i].address() as *mut PageTable) };

    let pd_i = pd_index(virt_addr);
    if !pd.entries[pd_i].is_present() { return; }
    let pt = unsafe { &mut *(pd.entries[pd_i].address() as *mut PageTable) };

    let pt_i = pt_index(virt_addr);
    pt.entries[pt_i] = PageEntry::empty();

    // Инвалидация TLB для этого адреса.
    // TLB (Translation Lookaside Buffer) — кэш трансляций в CPU.
    // После unmap нужно сбросить кэшированную трансляцию.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt_addr, options(nostack, preserves_flags));
    }
}

/// Обнулить фрейм (4096 байт). Используется при создании новых таблиц страниц.
fn zero_frame(addr: usize) {
    let ptr = addr as *mut u8;
    unsafe {
        for i in 0..PAGE_SIZE {
            ptr.add(i).write_volatile(0);
        }
    }
}
