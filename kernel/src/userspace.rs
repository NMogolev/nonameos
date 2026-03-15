// =============================================================================
// NoNameOS — User-Space Process Management
// =============================================================================
//
// Этот модуль отвечает за создание и запуск пользовательских процессов.
//
// Полный путь от PE-файла до выполнения в user-space:
//
//   1. PE Loader парсит файл, извлекает секции и entry point
//   2. userspace::create_process() создаёт новое адресное пространство (PML4)
//   3. Секции PE маппятся в user address space (map_user_pages)
//   4. Выделяется user stack
//   5. Создаётся Thread с entry point и user stack
//   6. При первом schedule → jump_to_usermode() через iretq
//
// Адресное пространство user-процесса:
//
//   0x0000_0000_0040_0000  — ImageBase (PE default)
//   ...                    — PE секции (.text, .data, .rdata)
//   0x0000_7FFF_FFFF_0000  — User stack top
//   0x0000_7FFF_FFFF_FFFF  — Конец user space
//   0xFFFF_8000_0000_0000+ — Kernel space (маппится во все процессы)
//
// Переход в Ring 3:
//   Используем iretq — CPU загружает SS:RSP, RFLAGS, CS:RIP из стека.
//   iretq подходит для первого входа в user-space.
//   Для возврата из syscall используем sysretq.
//
// Безопасность:
//   - User code НЕ может обращаться к kernel memory
//   - User code НЕ может выполнять привилегированные инструкции
//   - Единственный способ обратиться к ядру — syscall
// =============================================================================

use crate::memory::paging;
use crate::memory::phys;
use crate::gdt;
use crate::task::*;

// ---- Константы user-space layout ----

/// Базовый адрес загрузки PE (стандартный для Windows x64).
pub const USER_IMAGE_BASE: usize = 0x0000_0000_0040_0000;

/// Верхняя граница user stack (чуть ниже конца canonical user space).
pub const USER_STACK_TOP: usize = 0x0000_7FFF_FFFF_0000;

/// Размер user stack (64 KiB).
pub const USER_STACK_SIZE: usize = 16 * 4096;

/// Максимум страниц для одного PE образа.
pub const MAX_IMAGE_PAGES: usize = 256; // 1 MiB

// ---- Результат создания процесса ----

/// Информация о созданном user-процессе.
pub struct UserProcess {
    pub pid: Pid,
    pub tid: Tid,
    pub cr3: u64,
    pub entry_point: u64,
    pub stack_top: u64,
}

// ---- Создание адресного пространства ----

/// Создать новое адресное пространство (PML4) для user-процесса.
///
/// Новый PML4 содержит:
///   - Маппинг ядра (копируем верхнюю половину PML4 из текущего CR3)
///   - Пустую нижнюю половину (user space) — заполняется позже
///
/// Возвращает физический адрес нового PML4.
pub fn create_address_space() -> Option<u64> {
    // Выделяем фрейм для нового PML4
    let new_pml4_phys = phys::alloc_frame()?;

    // Обнуляем
    let new_pml4 = new_pml4_phys as *mut u64;
    unsafe {
        core::ptr::write_bytes(new_pml4 as *mut u8, 0, 4096);
    }

    // Копируем верхнюю половину PML4 (kernel mappings, entries 256..511)
    // из текущего адресного пространства.
    // Это гарантирует, что kernel code/data доступны из нового процесса.
    let current_cr3 = paging::read_cr3();
    let current_pml4 = current_cr3 as *const u64;

    unsafe {
        for i in 256..512 {
            let entry = current_pml4.add(i).read();
            new_pml4.add(i).write(entry);
        }
    }

    Some(new_pml4_phys as u64)
}

/// Замапить блок памяти в user address space.
///
/// Выделяет физические фреймы и маппит их по указанному виртуальному адресу.
/// Копирует `data` в замаплённые страницы.
///
/// `cr3` — физ. адрес PML4 процесса.
/// `virt_base` — виртуальный адрес начала маппинга (должен быть page-aligned).
/// `data` — данные для копирования (может быть короче выделенного размера).
/// `total_size` — общий размер маппинга (выравнивается вверх до PAGE_SIZE).
pub fn map_user_pages(
    cr3: u64,
    virt_base: usize,
    data: &[u8],
    total_size: usize,
) -> bool {
    let page_size = 4096;
    let num_pages = (total_size + page_size - 1) / page_size;

    if num_pages > MAX_IMAGE_PAGES {
        return false;
    }

    for i in 0..num_pages {
        let frame = match phys::alloc_frame() {
            Some(f) => f,
            None => return false,
        };

        // Обнуляем фрейм
        unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size); }

        // Копируем данные (если есть)
        let offset = i * page_size;
        let copy_start = offset;
        let copy_end = core::cmp::min(offset + page_size, data.len());
        if copy_start < data.len() {
            let len = copy_end - copy_start;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr().add(copy_start),
                    frame as *mut u8,
                    len,
                );
            }
        }

        // Маппим страницу с user=true
        let virt = virt_base + offset;
        paging::map_page(cr3, virt, frame, true);
    }

    true
}

/// Выделить user stack в адресном пространстве процесса.
///
/// Возвращает виртуальный адрес вершины стека (стек растёт вниз).
pub fn alloc_user_stack(cr3: u64) -> Option<u64> {
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;

    // Маппим страницы стека
    if !map_user_pages(cr3, stack_bottom, &[], USER_STACK_SIZE) {
        return None;
    }

    Some(USER_STACK_TOP as u64)
}

// ---- Создание user-процесса ----

/// Создать полноценный user-mode процесс.
///
/// `image_data` — raw bytes PE секций (уже обработанные лоадером).
/// `image_size` — размер образа в памяти.
/// `entry_rva` — RVA точки входа (относительно image base).
/// `name` — имя процесса.
///
/// Возвращает информацию о созданном процессе.
pub fn create_process(
    image_data: &[u8],
    image_size: usize,
    entry_rva: u32,
    name: &str,
) -> Option<UserProcess> {
    // 1. Создаём новое адресное пространство
    let cr3 = create_address_space()?;

    // 2. Маппим образ PE в user space
    if !map_user_pages(cr3, USER_IMAGE_BASE, image_data, image_size) {
        return None;
    }

    // 3. Выделяем user stack
    let stack_top = alloc_user_stack(cr3)?;

    // 4. Вычисляем абсолютный адрес entry point
    let entry_point = USER_IMAGE_BASE as u64 + entry_rva as u64;

    // 5. Создаём процесс в scheduler
    let pid = alloc_pid();
    let mut proc = Process::new(pid, cr3);
    proc.set_name(name);
    let _ = &proc; // Процесс будет зарегистрирован в scheduler в будущем

    // Регистрируем процесс (ищем свободный слот)
    // Пока используем простой подход через scheduler API
    // В будущем: отдельный process table

    // 6. Создаём главный поток
    let tid = alloc_tid();
    // Для user thread: entry и stack — user-space адреса.
    // При первом schedule поток должен вызвать jump_to_usermode().
    // Мы используем трюк: создаём kernel thread, который вызывает iretq.

    Some(UserProcess {
        pid,
        tid,
        cr3,
        entry_point,
        stack_top,
    })
}

// ---- Переход в Ring 3 ----

/// Прыгнуть в user-mode через iretq.
///
/// Устанавливает на стек фрейм для iretq:
///   [SS]     — user data segment (0x1B)
///   [RSP]    — user stack pointer
///   [RFLAGS] — flags с IF=1 (прерывания включены)
///   [CS]     — user code segment (0x23)
///   [RIP]    — user entry point
///
/// После iretq CPU переключается в Ring 3 и начинает исполнять user code.
///
/// ВНИМАНИЕ: эта функция НЕ ВОЗВРАЩАЕТСЯ. Она НЕ должна вызываться
/// из контекста, где нужен возврат.
pub fn jump_to_usermode(entry_point: u64, user_stack_top: u64, cr3: u64) -> ! {
    // Переключаем адресное пространство
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack));
    }

    // Обновляем TSS.RSP0 — стек ядра для этого потока
    // (при прерывании из user-mode CPU загрузит этот RSP)
    gdt::set_tss_rsp0(gdt::get_tss_rsp0());

    unsafe {
        core::arch::asm!(
            // SS (user data)
            "push {ss}",
            // RSP (user stack)
            "push {rsp_user}",
            // RFLAGS (IF=1, разрешаем прерывания)
            "push {rflags}",
            // CS (user code)
            "push {cs}",
            // RIP (entry point)
            "push {rip}",
            // Прыгаем!
            "iretq",
            ss = in(reg) gdt::USER_DATA_RPL3 as u64,
            rsp_user = in(reg) user_stack_top,
            rflags = in(reg) 0x202u64,  // IF=1, reserved bit 1=1
            cs = in(reg) gdt::USER_CODE_RPL3 as u64,
            rip = in(reg) entry_point,
            options(noreturn)
        );
    }
}

// ---- Демо: встроенный user-mode код ----

/// Минимальный user-mode код для тестирования.
///
/// Этот "binary" делает:
///   1. Записывает "Hello from userspace!\n" через sys_write(1, buf, len)
///   2. Вызывает sys_exit(0)
///
/// Машинный код x86_64 (позиционно-зависимый, ImageBase = 0x400000):
pub static DEMO_USER_CODE: &[u8] = &[
    // lea rsi, [rip + message]   ; адрес строки (offset +30 from next insn)
    0x48, 0x8D, 0x35, 0x1E, 0x00, 0x00, 0x00,

    // mov rdi, 1                 ; fd = 1 (stdout)
    0x48, 0xC7, 0xC7, 0x01, 0x00, 0x00, 0x00,

    // mov rdx, 22                ; len = 22
    0x48, 0xC7, 0xC2, 0x16, 0x00, 0x00, 0x00,

    // mov rax, 1                 ; syscall number = SYS_WRITE
    0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00,

    // syscall
    0x0F, 0x05,

    // mov rdi, 0                 ; exit code = 0
    0x48, 0xC7, 0xC7, 0x00, 0x00, 0x00, 0x00,

    // mov rax, 4                 ; syscall number = SYS_EXIT
    0x48, 0xC7, 0xC0, 0x04, 0x00, 0x00, 0x00,

    // syscall
    0x0F, 0x05,

    // jmp $ (safety net)
    0xEB, 0xFE,

    // "Hello from userspace!\n"
    b'H', b'e', b'l', b'l', b'o', b' ',
    b'f', b'r', b'o', b'm', b' ',
    b'u', b's', b'e', b'r', b's', b'p', b'a', b'c', b'e', b'!', b'\n',
];

/// Размер демо-образа в памяти (1 страница достаточно).
pub const DEMO_IMAGE_SIZE: usize = 4096;

/// RVA entry point для демо-кода (начало кода = 0).
pub const DEMO_ENTRY_RVA: u32 = 0;
