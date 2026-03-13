// =============================================================================
// NoNameOS — Object Manager
// =============================================================================
//
// Object Manager — ФУНДАМЕНТ NT. Буквально все в Windows — объект:
//
//   ┌─────────────────┬──────────────────────────────────────────────────┐
//   │ Тип объекта      │ Примеры                                         │
//   ├─────────────────┼──────────────────────────────────────────────────┤
//   │ File            │ C:\Windows\notepad.exe, \\.\COM1, pipe           │
//   │ Directory       │ \ObjectTypes, \Device, \BaseNamedObjects         │
//   │ SymbolicLink    │ C: → \Device\HarddiskVolume1                     │
//   │ Process         │ каждый запущенный .exe                           │
//   │ Thread          │ каждый поток в процессе                          │
//   │ Section         │ memory-mapped файл / shared memory               │
//   │ Mutant (Mutex)  │ CreateMutex() → именованный мьютекс              │
//   │ Event           │ CreateEvent() → сигнализация между потоками      │
//   │ Semaphore       │ CreateSemaphore()                                │
//   │ Key             │ ключ реестра (HKLM\Software\...)                 │
//   │ Token           │ security token (кто ты и какие права)            │
//   │ Timer           │ CreateWaitableTimer()                            │
//   │ IoCompletion    │ I/O Completion Port                              │
//   │ WindowStation   │ WinSta0 (рабочая станция окон)                   │
//   │ Desktop         │ Default (рабочий стол)                           │
//   └─────────────────┴──────────────────────────────────────────────────┘
//
// Каждый объект имеет:
//   - Тип (ObjectType) — определяет операции (open, close, delete...)
//   - Имя (опционально) — путь в Object Namespace (\Device\Harddisk0)
//   - Security Descriptor — кто имеет доступ
//   - Reference Count — сколько ссылок (хэндлов + указателей ядра)
//   - Handle Count — сколько процессов держат хэндл на этот объект
//
// Object Namespace — дерево каталогов (как файловая система):
//
//   \ (корень)
//   ├── ObjectTypes/       ← зарегистрированные типы объектов
//   │   ├── File
//   │   ├── Process
//   │   └── ...
//   ├── Device/            ← устройства
//   │   ├── HarddiskVolume1
//   │   ├── Null
//   │   └── ...
//   ├── BaseNamedObjects/  ← именованные объекты приложений
//   │   ├── MyAppMutex
//   │   └── SharedMemory1
//   ├── Sessions/          ← сессии пользователей
//   │   └── 0/
//   │       └── BaseNamedObjects/
//   ├── GLOBAL??/          ← символические ссылки (C:, D:, ...)
//   │   ├── C: → \Device\HarddiskVolume1
//   │   └── COM1 → \Device\Serial0
//   └── Registry/          ← корень реестра
//
// Handle Table:
//   Каждый процесс имеет свою таблицу хэндлов.
//   HANDLE — это индекс в этой таблице (× 4, т.к. младшие 2 бита зарезервированы).
//
//   Handle table процесса A:
//   ┌───────┬─────────────────────┬─────────────┐
//   │ Index │ Object              │ Access Mask │
//   ├───────┼─────────────────────┼─────────────┤
//   │ 0x04  │ → File(notepad.exe) │ READ        │
//   │ 0x08  │ → Event(my_event)   │ ALL         │
//   │ 0x0C  │ → (свободен)        │             │
//   │ 0x10  │ → Mutex(app_lock)   │ ALL         │
//   └───────┴─────────────────────┴─────────────┘
//
// Жизненный цикл:
//   1. NtCreateFile() → Object Manager создаёт File object
//   2. Добавляет запись в handle table процесса → возвращает HANDLE
//   3. Приложение использует HANDLE (ReadFile, WriteFile...)
//   4. CloseHandle() → удаляет запись из handle table, decrement ref count
//   5. Если ref count = 0 → объект уничтожается
//
// Аналоги:
//   ReactOS: ntoskrnl/ob/ (oblife.c, obhandle.c, obname.c, obdir.c)
//   Wine: server/object.c, server/handle.c, server/named_pipe.c
// =============================================================================

use spin::Mutex;
use super::types::*;
use super::error::*;

// ---- Типы объектов ----

/// Перечисление типов объектов NT.
/// Каждый тип определяет набор операций, которые можно выполнить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ObjectType {
    /// Каталог в Object Namespace.
    Directory = 0,

    /// Символическая ссылка (C: → \Device\HarddiskVolume1).
    SymbolicLink = 1,

    /// Файл, устройство, пайп, сокет.
    File = 2,

    /// Процесс.
    Process = 3,

    /// Поток.
    Thread = 4,

    /// Секция (memory-mapped file / shared memory).
    Section = 5,

    /// Мьютекс (в NT называется "Mutant").
    Mutant = 6,

    /// Событие (для синхронизации потоков).
    Event = 7,

    /// Семафор.
    Semaphore = 8,

    /// Таймер.
    Timer = 9,

    /// Ключ реестра.
    Key = 10,

    /// I/O Completion Port.
    IoCompletion = 11,

    /// Security Token.
    Token = 12,

    /// Window Station.
    WindowStation = 13,

    /// Desktop.
    Desktop = 14,
}

// ---- Object Header ----
//
// Каждый объект в ядре начинается с заголовка.
// За заголовком идут type-specific данные (файл, процесс, ...).

/// Заголовок объекта — общая часть для ВСЕХ типов объектов.
pub struct ObjectHeader {
    /// Тип объекта.
    pub object_type: ObjectType,

    /// Reference count: хэндлы + указатели ядра.
    /// Объект уничтожается когда ref_count == 0.
    pub ref_count: u32,

    /// Handle count: только хэндлы пользовательских процессов.
    pub handle_count: u32,

    /// Имя объекта (в Object Namespace). Пустое = анонимный объект.
    pub name: [u8; 128],
    pub name_len: usize,

    /// Флаги.
    pub permanent: bool,  // Не удалять при ref_count == 0

    /// Type-specific данные (union в C, мы используем enum).
    pub body: ObjectBody,
}

/// Тело объекта — данные, специфичные для типа.
pub enum ObjectBody {
    /// Пустое тело (для Directory, SymbolicLink и т.д.).
    None,

    /// File object — ссылка на VFS node + текущая позиция.
    File {
        /// Путь к файлу.
        path: [u8; 256],
        path_len: usize,
        /// Текущая позиция чтения/записи.
        position: u64,
        /// Режим доступа (ACCESS_MASK).
        access: ACCESS_MASK,
        /// Размер файла.
        size: u64,
    },

    /// Process object.
    Process {
        pid: u64,
        exit_code: u32,
        terminated: bool,
    },

    /// Thread object.
    Thread {
        tid: u64,
        owner_pid: u64,
        exit_code: u32,
        terminated: bool,
    },

    /// Mutant (Mutex).
    Mutant {
        /// Владелец (TID) или 0 = свободен.
        owner_tid: u64,
        /// Счётчик рекурсивных захватов.
        recursion_count: u32,
        /// Сигнальное состояние (true = свободен).
        signaled: bool,
    },

    /// Event.
    Event {
        /// Manual-reset (true) или auto-reset (false).
        manual_reset: bool,
        /// Текущее состояние (signaled = true).
        signaled: bool,
    },

    /// Semaphore.
    Semaphore {
        current_count: u32,
        maximum_count: u32,
    },

    /// Registry Key.
    Key {
        path: [u8; 256],
        path_len: usize,
    },

    /// Section (memory-mapped file / shared memory).
    Section {
        base_address: u64,
        size: u64,
    },
}

// ---- Handle Table ----
//
// Каждый процесс имеет свою таблицу хэндлов.
// Размер: фиксированный для простоты (в Windows — динамический).
//
// HANDLE = index * 4 (младшие 2 бита зарезервированы).
// Первый хэндл = 0x04 (индекс 1). Индекс 0 зарезервирован.

/// Запись в таблице хэндлов.
#[derive(Clone)]
pub struct HandleEntry {
    /// Индекс объекта в глобальном пуле (или u32::MAX если свободна).
    pub object_index: u32,

    /// Права доступа, с которыми был открыт хэндл.
    pub access_mask: ACCESS_MASK,

    /// Флаги хэндла.
    pub inherit: bool,    // Наследуется дочерними процессами?
    pub protect: bool,    // Защищён от CloseHandle()?
}

impl HandleEntry {
    pub const fn empty() -> Self {
        HandleEntry {
            object_index: u32::MAX,
            access_mask: 0,
            inherit: false,
            protect: false,
        }
    }

    pub fn is_free(&self) -> bool {
        self.object_index == u32::MAX
    }
}

/// Размер таблицы хэндлов (на один процесс).
/// Windows поддерживает до ~16 млн хэндлов. Мы начинаем с 256.
const HANDLE_TABLE_SIZE: usize = 256;

/// Таблица хэндлов процесса.
pub struct HandleTable {
    entries: [HandleEntry; HANDLE_TABLE_SIZE],
}

impl HandleTable {
    pub const fn new() -> Self {
        HandleTable {
            entries: [const { HandleEntry::empty() }; HANDLE_TABLE_SIZE],
        }
    }

    /// Выделить новый хэндл → HANDLE.
    /// Возвращает HANDLE или INVALID_HANDLE_VALUE.
    pub fn alloc(&mut self, object_index: u32, access: ACCESS_MASK) -> HANDLE {
        // Начинаем с индекса 1 (индекс 0 = зарезервирован)
        for i in 1..HANDLE_TABLE_SIZE {
            if self.entries[i].is_free() {
                self.entries[i] = HandleEntry {
                    object_index,
                    access_mask: access,
                    inherit: false,
                    protect: false,
                };
                // HANDLE = index * 4
                return HANDLE(i * 4);
            }
        }
        HANDLE::INVALID
    }

    /// Получить запись по HANDLE.
    pub fn get(&self, handle: HANDLE) -> Option<&HandleEntry> {
        let index = handle.0 / 4;
        if index == 0 || index >= HANDLE_TABLE_SIZE {
            return None;
        }
        let entry = &self.entries[index];
        if entry.is_free() {
            return None;
        }
        Some(entry)
    }

    /// Закрыть хэндл (освободить запись).
    pub fn close(&mut self, handle: HANDLE) -> NTSTATUS {
        let index = handle.0 / 4;
        if index == 0 || index >= HANDLE_TABLE_SIZE {
            return STATUS_INVALID_HANDLE;
        }
        if self.entries[index].is_free() {
            return STATUS_INVALID_HANDLE;
        }
        if self.entries[index].protect {
            return STATUS_HANDLE_NOT_CLOSABLE;
        }
        self.entries[index] = HandleEntry::empty();
        STATUS_SUCCESS
    }

    /// Дублировать хэндл (DuplicateHandle).
    pub fn duplicate(&mut self, source: HANDLE, new_access: ACCESS_MASK) -> HANDLE {
        let src_index = source.0 / 4;
        if src_index == 0 || src_index >= HANDLE_TABLE_SIZE {
            return HANDLE::INVALID;
        }
        if self.entries[src_index].is_free() {
            return HANDLE::INVALID;
        }
        let obj_index = self.entries[src_index].object_index;
        self.alloc(obj_index, new_access)
    }
}

const STATUS_HANDLE_NOT_CLOSABLE: NTSTATUS = NTSTATUS(0xC0000235);

// ---- Глобальный пул объектов ----
//
// Все объекты ядра хранятся в глобальном массиве.
// Handle table каждого процесса ссылается на индексы в этом массиве.
//
// В реальной ОС объекты аллоцируются динамически из pool memory.
// Мы используем статический массив для простоты.

const MAX_OBJECTS: usize = 1024;

struct ObjectPool {
    objects: [Option<ObjectHeader>; MAX_OBJECTS],
    count: usize,
}

/// Глобальный пул объектов ядра.
/// Защищён спинлоком для многопроцессорной безопасности.
static OBJECT_POOL: Mutex<ObjectPool> = Mutex::new(ObjectPool {
    // SAFETY: Option<ObjectHeader> не реализует Copy, поэтому используем const block
    objects: [const { None }; MAX_OBJECTS],
    count: 0,
});

// ---- Публичный API ----

/// Создать новый объект в глобальном пуле.
/// Возвращает индекс объекта или None если пул полон.
pub fn create_object(obj_type: ObjectType, name: &str, body: ObjectBody) -> Option<u32> {
    let mut pool = OBJECT_POOL.lock();

    for i in 0..MAX_OBJECTS {
        if pool.objects[i].is_none() {
            let mut header = ObjectHeader {
                object_type: obj_type,
                ref_count: 1,
                handle_count: 0,
                name: [0; 128],
                name_len: 0,
                permanent: false,
                body,
            };

            // Копируем имя
            let name_bytes = name.as_bytes();
            let len = core::cmp::min(name_bytes.len(), 127);
            header.name[..len].copy_from_slice(&name_bytes[..len]);
            header.name_len = len;

            pool.objects[i] = Some(header);
            pool.count += 1;
            return Some(i as u32);
        }
    }
    None
}

/// Увеличить reference count объекта.
pub fn reference_object(index: u32) -> NTSTATUS {
    let mut pool = OBJECT_POOL.lock();
    if let Some(Some(obj)) = pool.objects.get_mut(index as usize) {
        obj.ref_count += 1;
        STATUS_SUCCESS
    } else {
        STATUS_INVALID_HANDLE
    }
}

/// Уменьшить reference count. Если стал 0 — удалить объект.
pub fn dereference_object(index: u32) -> NTSTATUS {
    let mut pool = OBJECT_POOL.lock();
    let slot = match pool.objects.get_mut(index as usize) {
        Some(slot) => slot,
        None => return STATUS_INVALID_HANDLE,
    };

    match slot {
        Some(obj) => {
            if obj.ref_count == 0 {
                return STATUS_INVALID_HANDLE;
            }
            obj.ref_count -= 1;
            if obj.ref_count == 0 && !obj.permanent {
                *slot = None;
                pool.count -= 1;
            }
            STATUS_SUCCESS
        }
        None => STATUS_INVALID_HANDLE,
    }
}

/// Найти объект по имени (для NtOpenFile, NtOpenKey и т.д.).
pub fn lookup_object(name: &str) -> Option<u32> {
    let name_bytes = name.as_bytes();
    let pool = OBJECT_POOL.lock();

    for i in 0..MAX_OBJECTS {
        if let Some(obj) = &pool.objects[i] {
            if obj.name_len == name_bytes.len()
                && &obj.name[..obj.name_len] == name_bytes
            {
                return Some(i as u32);
            }
        }
    }
    None
}

/// Получить тип объекта по индексу.
pub fn get_object_type(index: u32) -> Option<ObjectType> {
    let pool = OBJECT_POOL.lock();
    pool.objects.get(index as usize)
        .and_then(|slot| slot.as_ref())
        .map(|obj| obj.object_type)
}

/// Количество объектов в пуле (для отладки).
pub fn object_count() -> usize {
    OBJECT_POOL.lock().count
}

// ---- Операции высокого уровня ----

/// Открыть объект и получить HANDLE (NtOpenObject-стиль).
///
/// 1. Находит объект по имени
/// 2. Проверяет тип
/// 3. Увеличивает ref_count
/// 4. Создаёт запись в handle table процесса
/// 5. Возвращает HANDLE
pub fn open_object(
    handle_table: &mut HandleTable,
    name: &str,
    expected_type: ObjectType,
    access: ACCESS_MASK,
) -> Result<HANDLE, NTSTATUS> {
    // Ищем объект
    let index = lookup_object(name).ok_or(STATUS_OBJECT_NAME_NOT_FOUND)?;

    // Проверяем тип
    let obj_type = get_object_type(index).ok_or(STATUS_INVALID_HANDLE)?;
    if obj_type != expected_type {
        return Err(STATUS_OBJECT_TYPE_MISMATCH);
    }

    // Увеличиваем ref count
    let status = reference_object(index);
    if status.is_error() {
        return Err(status);
    }

    // Создаём хэндл
    let handle = handle_table.alloc(index, access);
    if handle.is_invalid() {
        dereference_object(index); // откатываем
        return Err(STATUS_INSUFFICIENT_RESOURCES);
    }

    Ok(handle)
}

/// Закрыть HANDLE (NtClose).
///
/// 1. Находит запись в handle table
/// 2. Уменьшает ref_count объекта
/// 3. Удаляет запись из handle table
pub fn close_handle(handle_table: &mut HandleTable, handle: HANDLE) -> NTSTATUS {
    // Получаем object index из handle table
    let entry = match handle_table.get(handle) {
        Some(e) => e.clone(),
        None => return STATUS_INVALID_HANDLE,
    };

    // Закрываем хэндл в таблице
    let status = handle_table.close(handle);
    if status.is_error() {
        return status;
    }

    // Уменьшаем ref count объекта
    dereference_object(entry.object_index)
}
