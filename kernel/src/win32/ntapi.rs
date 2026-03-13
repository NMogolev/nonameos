// =============================================================================
// NoNameOS — NT Native API
// =============================================================================
//
// NT Native API — это интерфейс Windows ядра.
// Известно как "Win32 API" —
// это обёртки из KERNEL32.DLL, которые ВНУТРИ вызывают NT API.
//
// Цепочка вызова для CreateFileW("C:\\test.txt"):
//
//   app.exe
//     → KERNEL32!CreateFileW()          ← Win32 API (то, что знают программисты)
//       → NTDLL!NtCreateFile()          ← NT Native API (переход в ядро)
//         → syscall инструкция          ← аппаратный переход Ring 3 → Ring 0
//           → NTOSKRNL!NtCreateFile()   ← обработчик в ядре
//             → I/O Manager             ← запрос к файловой системе
//               → Driver (NTFS)         ← драйвер ФС
//
// Почему важен NT API, а не Win32:
//
//   1. Win32 API — это слишком много функций. NT API поменьше.
//      Реализовав все NT функции, мы покрываем все Win32 функции,
//      потому что Win32 — просто тонкие обёртки.
//
//   2. NT API стабилен: Microsoft почти не меняет его между версиями Windows.
//      Win32 API добавляет новые функции, но они все зовут те же Nt*.
//
//   3. NT API ближе к нашему микроядру: он оперирует объектами, хэндлами,
//      статусами — теми же концепциями, что мы уже определили.
//
// Конвенции NT API:
//   - Все функции начинаются с Nt или Zw:
//       NtCreateFile() — user-mode версия (проверяет параметры)
//       ZwCreateFile() — kernel-mode версия (доверяет параметрам)
//     В нашем ядре разницы нет.
//
//   - Возвращают NTSTATUS.
//
//   - Принимают OBJECT_ATTRIBUTES для описания объекта.
//
//   - Выходные параметры через указатели (а не через return).
//
//   - Строки — UNICODE_STRING (не null-terminated).
//
// Приоритет реализации:
//
//   MUST HAVE:
//     NtCreateFile / NtOpenFile / NtReadFile / NtWriteFile / NtClose
//     NtAllocateVirtualMemory / NtFreeVirtualMemory
//     NtCreateProcess / NtCreateThread / NtTerminateProcess
//     NtCreateEvent / NtSetEvent / NtWaitForSingleObject
//     NtQueryInformationProcess / NtQueryInformationThread
//
//   IMPORTANT:
//     NtCreateSection / NtMapViewOfSection (DLL loading, shared memory)
//     NtCreateMutant / NtReleaseMutant
//     NtOpenKey / NtQueryValueKey (реестр)
//     NtQuerySystemInformation
//     NtDelayExecution (Sleep)
//     NtDuplicateObject
//
//   NICE TO HAVE:
//     NtCreateNamedPipeFile, NtCreateMailslotFile
//     NtNotifyChangeKey (filesystem watch)
//     NtCreateIoCompletion (IOCP)
//
// Источники:
//   - Wine: dlls/ntdll/*.c (реализация каждой Nt* функции)
//   - ReactOS: ntoskrnl/io/, ntoskrnl/mm/, ntoskrnl/ps/, ntoskrnl/ob/
//   - Undocumented NT: http://undocumented.ntinternals.net/
//   - PHNT headers: https://github.com/processhacker/phnt
// =============================================================================

use super::types::*;
use super::error::*;
use super::object;

// =============================================================================
// ФАЙЛОВЫЕ ОПЕРАЦИИ
// =============================================================================

/// NtCreateFile — создать или открыть файл/устройство/пайп.
///
/// Это самая важная функция файловой подсистемы.
/// CreateFileW(), fopen(), open() — все сводятся к ней.
///
/// Параметры:
///   file_handle     — [out] хэндл созданного/открытого файла
///   desired_access  — права доступа (GENERIC_READ, GENERIC_WRITE...)
///   object_attrs    — имя файла, корневой каталог, флаги
///   io_status       — [out] результат операции + кол-во байт
///   allocation_size — начальный размер (для создания)
///   file_attributes — атрибуты файла (READONLY, HIDDEN, SYSTEM...)
///   share_access    — совместный доступ (FILE_SHARE_READ | WRITE | DELETE)
///   create_disposition — что делать если файл существует/не существует:
///     FILE_SUPERSEDE  (0) — удалить и создать новый
///     FILE_OPEN       (1) — открыть (ошибка если не существует)
///     FILE_CREATE     (2) — создать (ошибка если существует)
///     FILE_OPEN_IF    (3) — открыть или создать
///     FILE_OVERWRITE  (4) — открыть и обнулить (ошибка если не существует)
///     FILE_OVERWRITE_IF (5) — открыть и обнулить, или создать
///   create_options  — флаги (DIRECTORY_FILE, NON_DIRECTORY_FILE, SYNCHRONOUS...)
///   ea_buffer       — Extended Attributes (обычно NULL)
///   ea_length       — размер EA
pub fn nt_create_file(
    handle_table: &mut object::HandleTable,
    _desired_access: ACCESS_MASK,
    object_attrs: &OBJECT_ATTRIBUTES,
    io_status: &mut IO_STATUS_BLOCK,
    _allocation_size: Option<u64>,
    _file_attributes: DWORD,
    _share_access: DWORD,
    create_disposition: DWORD,
    _create_options: DWORD,
) -> NTSTATUS {
    if object_attrs.object_name.is_null() {
        io_status.status = STATUS_OBJECT_NAME_INVALID;
        return STATUS_OBJECT_NAME_INVALID;
    }

    // TODO: VFS
    // Это заглушка, которая создаёт объект File в Object Manager

    let _disposition = create_disposition;

    // Создаём File object
    let obj_index = match object::create_object(
        object::ObjectType::File,
        "", // имя будет из UNICODE_STRING
        object::ObjectBody::File {
            path: [0; 256],
            path_len: 0,
            position: 0,
            access: _desired_access,
            size: 0,
        },
    ) {
        Some(idx) => idx,
        None => {
            io_status.status = STATUS_INSUFFICIENT_RESOURCES;
            return STATUS_INSUFFICIENT_RESOURCES;
        }
    };

    let handle = handle_table.alloc(obj_index, _desired_access);
    if handle.is_invalid() {
        object::dereference_object(obj_index);
        io_status.status = STATUS_INSUFFICIENT_RESOURCES;
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    io_status.status = STATUS_SUCCESS;
    io_status.information = 0; // FILE_OPENED / FILE_CREATED / etc.

    // NOTE: handle нужно передать caller'у через out-parameter.
    // Потом file_handle будет *mut HANDLE.
    let _ = handle;

    STATUS_SUCCESS
}

// Константы create_disposition
pub const FILE_SUPERSEDE: DWORD    = 0;
pub const FILE_OPEN: DWORD        = 1;
pub const FILE_CREATE: DWORD      = 2;
pub const FILE_OPEN_IF: DWORD     = 3;
pub const FILE_OVERWRITE: DWORD   = 4;
pub const FILE_OVERWRITE_IF: DWORD = 5;

// Константы share_access
pub const FILE_SHARE_READ: DWORD   = 0x00000001;
pub const FILE_SHARE_WRITE: DWORD  = 0x00000002;
pub const FILE_SHARE_DELETE: DWORD = 0x00000004;

// Константы file_attributes
pub const FILE_ATTRIBUTE_READONLY: DWORD  = 0x00000001;
pub const FILE_ATTRIBUTE_HIDDEN: DWORD    = 0x00000002;
pub const FILE_ATTRIBUTE_SYSTEM: DWORD    = 0x00000004;
pub const FILE_ATTRIBUTE_DIRECTORY: DWORD = 0x00000010;
pub const FILE_ATTRIBUTE_ARCHIVE: DWORD   = 0x00000020;
pub const FILE_ATTRIBUTE_NORMAL: DWORD    = 0x00000080;

// =============================================================================
// ПАМЯТЬ
// =============================================================================

/// NtAllocateVirtualMemory — выделить виртуальную память в адресном пространстве процесса.
///
/// Это то, что стоит за VirtualAlloc(), malloc(), HeapAlloc().
///
/// Может:
///   - Зарезервировать регион (MEM_RESERVE) — адресное пространство занято, но память не выделена
///   - Зафиксировать регион (MEM_COMMIT) — физическая память привязана
///   - Оба сразу (MEM_RESERVE | MEM_COMMIT)
///
/// Параметры:
///   process_handle — хэндл процесса (или текущий = HANDLE(-1))
///   base_address   — [in/out] желаемый адрес (0 = выбрать автоматически)
///   region_size    — [in/out] размер в байтах (выравнивается до страницы)
///   allocation_type — MEM_RESERVE, MEM_COMMIT, или оба
///   protect        — защита страниц (PAGE_READWRITE, PAGE_EXECUTE_READ...)
pub fn nt_allocate_virtual_memory(
    _process_handle: HANDLE,
    base_address: &mut u64,
    region_size: &mut u64,
    allocation_type: DWORD,
    protect: DWORD,
) -> NTSTATUS {
    let _ = allocation_type;
    let _ = protect;

    // Выравниваем размер до страницы
    let size = ((*region_size + 0xFFF) & !0xFFF) as usize;
    *region_size = size as u64;

    // TODO: сделать через memory manager
    // Это заглушка

    if *base_address == 0 {
        // Автоматический выбор адреса
        // В будущем: поиск свободного региона в адресном пространстве процесса
        *base_address = 0x10000; // заглушка
    }

    STATUS_SUCCESS
}

pub fn nt_free_virtual_memory(
    _process_handle: HANDLE,
    base_address: &mut u64,
    region_size: &mut u64,
    free_type: DWORD,
) -> NTSTATUS {
    let _ = free_type;
    let _ = base_address;
    let _ = region_size;

    // TODO: Доделать
    STATUS_SUCCESS
}

// Константы allocation_type
pub const MEM_COMMIT: DWORD      = 0x00001000;
pub const MEM_RESERVE: DWORD     = 0x00002000;
pub const MEM_DECOMMIT: DWORD    = 0x00004000;
pub const MEM_RELEASE: DWORD     = 0x00008000;
pub const MEM_RESET: DWORD       = 0x00080000;

// Константы protect
pub const PAGE_NOACCESS: DWORD          = 0x01;
pub const PAGE_READONLY: DWORD          = 0x02;
pub const PAGE_READWRITE: DWORD         = 0x04;
pub const PAGE_WRITECOPY: DWORD         = 0x08;
pub const PAGE_EXECUTE: DWORD           = 0x10;
pub const PAGE_EXECUTE_READ: DWORD      = 0x20;
pub const PAGE_EXECUTE_READWRITE: DWORD = 0x40;
pub const PAGE_GUARD: DWORD             = 0x100;

// =============================================================================
// ПРОЦЕССЫ И ПОТОКИ
// =============================================================================

/// NtClose — закрыть любой хэндл.
///
/// Это ЕДИНСТВЕННАЯ функция для закрытия хэндлов.
/// CloseHandle() из KERNEL32 — просто обёртка вокруг NtClose().
pub fn nt_close(
    handle_table: &mut object::HandleTable,
    handle: HANDLE,
) -> NTSTATUS {
    object::close_handle(handle_table, handle)
}

/// NtTerminateProcess — завершить процесс.
pub fn nt_terminate_process(
    _process_handle: HANDLE,
    _exit_status: NTSTATUS,
) -> NTSTATUS {
    // TODO: Доделать
    // 1. Пометить все потоки процесса как Dead
    // 2. Закрыть все хэндлы
    // 3. Освободить адресное пространство
    // 4. Записать exit code
    // 5. Сигнализировать ожидающим (NtWaitForSingleObject)
    STATUS_SUCCESS
}

// =============================================================================
// СИНХРОНИЗАЦИЯ
// =============================================================================

/// NtCreateEvent — создать объект-событие (для синхронизации потоков).
///
/// Event — самый простой примитив синхронизации:
///   Signaled = поток может продолжить
///   Non-signaled = поток ждёт
///
///   Manual Reset: SetEvent() → signaled; ResetEvent() → non-signaled
///                 Все ждущие потоки просыпаются.
///
///   Auto Reset: SetEvent() → один ждущий поток просыпается,
///               событие автоматически сбрасывается.
pub fn nt_create_event(
    handle_table: &mut object::HandleTable,
    desired_access: ACCESS_MASK,
    object_attrs: Option<&OBJECT_ATTRIBUTES>,
    manual_reset: bool,
    initial_state: bool,
) -> Result<HANDLE, NTSTATUS> {
    let name = if let Some(attrs) = object_attrs {
        // TODO: извлечь имя из UNICODE_STRING
        let _ = attrs;
        ""
    } else {
        ""
    };

    let obj_index = object::create_object(
        object::ObjectType::Event,
        name,
        object::ObjectBody::Event {
            manual_reset,
            signaled: initial_state,
        },
    ).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

    let handle = handle_table.alloc(obj_index, desired_access);
    if handle.is_invalid() {
        object::dereference_object(obj_index);
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    Ok(handle)
}

/// NtCreateMutant — создать мьютекс.
///
/// Мьютекс — примитив взаимного исключения:
///   Только ОДИН поток может владеть мьютексом.
///   Другие потоки блокируются до освобождения.
///   Поддерживает рекурсивный захват (один поток может захватить несколько раз).
pub fn nt_create_mutant(
    handle_table: &mut object::HandleTable,
    desired_access: ACCESS_MASK,
    _object_attrs: Option<&OBJECT_ATTRIBUTES>,
    initial_owner: bool,
) -> Result<HANDLE, NTSTATUS> {
    let obj_index = object::create_object(
        object::ObjectType::Mutant,
        "",
        object::ObjectBody::Mutant {
            owner_tid: if initial_owner { 1 } else { 0 }, // TODO: real TID
            recursion_count: if initial_owner { 1 } else { 0 },
            signaled: !initial_owner,
        },
    ).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

    let handle = handle_table.alloc(obj_index, desired_access);
    if handle.is_invalid() {
        object::dereference_object(obj_index);
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    Ok(handle)
}

/// NtWaitForSingleObject — ждать, пока объект станет в signaled.
///
/// основная функция ожидания. WaitForSingleObject() — обёртка.
/// Работает с: Event, Mutex, Semaphore, Process, Thread, Timer.
///
/// timeout: None = бесконечное ожидание
///          Some(0) = проверить и вернуть немедленно
///          Some(n) = ждать n * 100ns
pub fn nt_wait_for_single_object(
    _handle: HANDLE,
    _alertable: bool,
    _timeout: Option<i64>,
) -> NTSTATUS {
    // TODO: Что делаем:
    // 1. Получить объект по хэндлу
    // 2. Проверить, является ли он waitable
    // 3. Если signaled — вернуть STATUS_SUCCESS
    // 4. Если нет — заблокировать текущий поток
    // 5. Планировщик разбудит когда объект станет signaled или timeout

    STATUS_SUCCESS
}

/// NtDelayExecution — усыпить текущий поток
///
/// delay: отрицательное = относительное время
///        положительное = абсолютное время
///
/// Sleep(1000) = NtDelayExecution(FALSE, -10000000)  // 1 сек = 10^7 * 100ns
pub fn nt_delay_execution(
    _alertable: bool,
    _delay: i64,
) -> NTSTATUS {
    // TODO: Сделать через PIT таймер + планировщик
    STATUS_SUCCESS
}

// =============================================================================
// ИНФОРМАЦИОННЫЕ ЗАПРОСЫ
// =============================================================================

/// Классы информации о процессе (для NtQueryInformationProcess).
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum ProcessInfoClass {
    ProcessBasicInformation = 0,
    ProcessQuotaLimits = 1,
    ProcessIoCounters = 2,
    ProcessVmCounters = 3,
    ProcessTimes = 4,
    ProcessImageFileName = 27,
    ProcessImageInformation = 44,
}

/// Базовая информация о процессе.
#[repr(C)]
pub struct ProcessBasicInformation {
    pub exit_status: NTSTATUS,
    pub peb_base_address: u64,      // Адрес PEB
    pub affinity_mask: u64,
    pub base_priority: i32,
    pub unique_process_id: u64,
    pub inherited_from_process_id: u64,
}

/// NtQueryInformationProcess — получить информацию о процессе.
pub fn nt_query_information_process(
    _process_handle: HANDLE,
    _info_class: ProcessInfoClass,
    _buffer: *mut u8,
    _buffer_size: DWORD,
    _return_length: *mut DWORD,
) -> NTSTATUS {
    // TODO: Доработка надо
    STATUS_NOT_IMPLEMENTED
}

// =============================================================================
// РЕЕСТР
// =============================================================================
//
// Реестр Windows — иерархическая база данных конфигурации.
// Ключи содержат значения таких типов:
//   REG_SZ         — строка (UTF-16)
//   REG_DWORD      — 32-bit число
//   REG_QWORD      — 64-bit число
//   REG_BINARY     — произвольные байты
//   REG_MULTI_SZ   — массив строк
//   REG_EXPAND_SZ  — строка с переменными окружения (%PATH%)
//
// Корневые ключи:
//   HKEY_LOCAL_MACHINE  (HKLM) — общесистемные настройки
//   HKEY_CURRENT_USER   (HKCU) — настройки текущего пользователя
//   HKEY_CLASSES_ROOT   (HKCR) — файловые ассоциации, COM
//   HKEY_USERS          (HKU)  — все пользователи

pub const REG_NONE: DWORD              = 0;
pub const REG_SZ: DWORD               = 1;
pub const REG_EXPAND_SZ: DWORD        = 2;
pub const REG_BINARY: DWORD           = 3;
pub const REG_DWORD: DWORD            = 4;
pub const REG_DWORD_BIG_ENDIAN: DWORD = 5;
pub const REG_LINK: DWORD             = 6;
pub const REG_MULTI_SZ: DWORD         = 7;
pub const REG_QWORD: DWORD            = 11;

/// NtOpenKey — открыть ключ реестра.
pub fn nt_open_key(
    handle_table: &mut object::HandleTable,
    desired_access: ACCESS_MASK,
    object_attrs: &OBJECT_ATTRIBUTES,
) -> Result<HANDLE, NTSTATUS> {
    let _ = object_attrs;

    // TODO: Registry subsystem

    let obj_index = object::create_object(
        object::ObjectType::Key,
        "",
        object::ObjectBody::Key {
            path: [0; 256],
            path_len: 0,
        },
    ).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

    let handle = handle_table.alloc(obj_index, desired_access);
    if handle.is_invalid() {
        object::dereference_object(obj_index);
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    Ok(handle)
}

// =============================================================================
// SECTION (Memory-Mapped Files / DLL Loading)
// =============================================================================
//
// Section — ключевой механизм для загрузки DLL:
//   1. NtCreateSection() — создаёт "секцию" из файла
//   2. NtMapViewOfSection() — мапит секцию в адресное пространство процесса
//
// Когда LoadLibrary("user32.dll"):
//   → NtOpenFile() открывает файл
//   → NtCreateSection() создаёт секцию типа SEC_IMAGE
//   → NtMapViewOfSection() мапит PE-образ в память
//   → Обрабатываются relocations и imports
//   → Вызывается DllMain()

/// NtCreateSection — создать объект-секцию.
pub fn nt_create_section(
    handle_table: &mut object::HandleTable,
    desired_access: ACCESS_MASK,
    _object_attrs: Option<&OBJECT_ATTRIBUTES>,
    maximum_size: u64,
    _section_page_protection: DWORD,
    _allocation_attributes: DWORD,
    _file_handle: HANDLE,
) -> Result<HANDLE, NTSTATUS> {
    let obj_index = object::create_object(
        object::ObjectType::Section,
        "",
        object::ObjectBody::Section {
            base_address: 0,
            size: maximum_size,
        },
    ).ok_or(STATUS_INSUFFICIENT_RESOURCES)?;

    let handle = handle_table.alloc(obj_index, desired_access);
    if handle.is_invalid() {
        object::dereference_object(obj_index);
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    Ok(handle)
}

// Section allocation attributes
pub const SEC_COMMIT: DWORD  = 0x08000000;
pub const SEC_RESERVE: DWORD = 0x04000000;
pub const SEC_IMAGE: DWORD   = 0x01000000;  // PE image
