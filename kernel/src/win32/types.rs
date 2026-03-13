// =============================================================================
// NoNameOS — Фундаментальные типы Windows NT / Win32
// =============================================================================
//
// Этот файл — САМЫЙ ВАЖНЫЙ в слое совместимости.
// Каждый Win32 API использует эти типы. Определив их один раз,
// мы можем писать любые API-функции в точности как в Windows.
//
// Источники:
//   - Microsoft SDK: minwindef.h, winnt.h, basetsd.h, ntstatus.h
//   - Wine: include/windef.h, include/winnt.h, include/basetsd.h
//   - ReactOS: sdk/include/reactos/wine/winternl.h
//
// Зачем свои типы, а не просто u32/i32?
//   1. Совместимость: Win32 API ожидает DWORD, а не u32. Мы должны
//      точно повторить сигнатуры, чтобы .exe вызывал наши функции.
//   2. Документация: HANDLE говорит «это дескриптор объекта»,
//      а usize — ничего не говорит.
//   3. Размеры: DWORD всегда 32 бита, даже на 64-битной системе.
//      Win32 ABI строго определяет размеры.
//
// Конвенции именования Windows:
//   - Все типы КАПСОМ: DWORD, HANDLE, BOOL
//   - Указатели с префиксом P/LP: PDWORD, LPVOID
//   - Длинные указатели (LP) — наследие 16-bit Windows, сейчас = P
//   - W-суффикс = Wide (UTF-16): LPWSTR, LPCWSTR
//   - A-суффикс = ANSI (8-bit): LPSTR, LPCSTR
//   - Windows внутренне использует UTF-16 (Wide), ANSI — для обратной совместимости
// =============================================================================

// ---- Базовые целочисленные типы ----
//
// Windows определяет свои типы поверх C-типов.
// Размеры фиксированы ABI и НЕ зависят от платформы
// (DWORD = 32 бита и на x86, и на x64, и на ARM).

/// 8-bit unsigned (0..255). Один байт.
/// Используется для: raw data, символы ANSI, флаги.
pub type BYTE = u8;

/// 16-bit unsigned (0..65535). «Слово» в терминологии Intel.
/// Используется для: порты I/O, старые API, char codes.
pub type WORD = u16;

/// 32-bit unsigned (0..4294967295). «Двойное слово».
/// THE самый частый тип в Win32 API.
/// Используется для: размеры, флаги, коды ошибок, цвета, ID процессов.
pub type DWORD = u32;

/// 64-bit unsigned. «Четверное слово».
/// Используется для: размеры файлов, timestamps, адреса в 64-bit.
pub type QWORD = u64;

/// 32-bit signed.
/// Используется для: координаты окон, счётчики, возвращаемые значения.
pub type LONG = i32;

/// 64-bit signed.
pub type LONGLONG = i64;

/// 32-bit unsigned (синоним DWORD, но семантически — unsigned LONG).
pub type ULONG = u32;

/// 64-bit unsigned.
pub type ULONGLONG = u64;

/// 16-bit signed.
pub type SHORT = i16;

/// 16-bit unsigned.
pub type USHORT = u16;

/// 8-bit signed.
pub type CHAR = i8;

/// 8-bit unsigned (синоним BYTE).
pub type UCHAR = u8;

/// 16-bit unsigned — «широкий символ» (UTF-16 code unit).
/// Windows ВНУТРЕННЕ работает с UTF-16. Каждый символ — 2 байта.
/// Русская 'Я' = 0x042F, английская 'A' = 0x0041.
/// Эмодзи и редкие символы = суррогатная пара (2 × WCHAR).
pub type WCHAR = u16;

// ---- Типы, зависящие от разрядности ----
//
// Эти типы меняют размер в зависимости от 32/64-bit.
// Мы — x86_64, поэтому они 64-битные.

/// Pointer-sized unsigned integer.
/// 32 бита на x86, 64 бита на x64.
/// Используется для: арифметика с указателями, размеры буферов.
pub type ULONG_PTR = usize;

/// Pointer-sized signed integer.
pub type LONG_PTR = isize;

/// Размер (в байтах). Pointer-sized.
/// Аналог Rust usize.
pub type SIZE_T = usize;

/// Signed size.
pub type SSIZE_T = isize;

/// Pointer-sized unsigned (синоним ULONG_PTR).
pub type DWORD_PTR = usize;

/// Параметры оконных сообщений:
///   WPARAM — «word parameter» (исторически 16-bit, сейчас pointer-sized)
///   LPARAM — «long parameter» (всегда pointer-sized)
///
/// В SendMessage(hwnd, msg, wParam, lParam):
///   wParam — обычно числовой параметр (ID команды, код клавиши...)
///   lParam — обычно указатель или составное значение
pub type WPARAM = usize;
pub type LPARAM = isize;

/// Результат обработки оконного сообщения.
pub type LRESULT = isize;

// ---- BOOL ----
//
// Windows BOOL — это int (4 байта), НЕ 1 байт!
// TRUE = 1, FALSE = 0, но функции могут вернуть любое ненулевое значение.
// Поэтому проверяют `if (result)`, а не `if (result == TRUE)`.

/// Windows BOOL — 32-bit integer. 0 = FALSE, != 0 = TRUE.
pub type BOOL = i32;

pub const TRUE: BOOL = 1;
pub const FALSE: BOOL = 0;

// ---- HANDLE ----
//
// HANDLE — центральная абстракция Windows.
// Это индекс в таблице объектов процесса (handle table).
//
// Когда приложение вызывает CreateFile(), ядро:
//   1. Создаёт объект File в kernel space
//   2. Добавляет ссылку в handle table процесса
//   3. Возвращает HANDLE (индекс * 4, для исторических причин)
//
// Приложение НЕ видит объект напрямую — только через HANDLE.
// Это обеспечивает изоляцию: процесс не может получить доступ
// к объектам, на которые у него нет HANDLE.
//
// Специальные значения:
//   INVALID_HANDLE_VALUE = -1 (0xFFFFFFFFFFFFFFFF) — ошибка
//   NULL = 0 — «нет хэндла»
//   Псевдо-хэндлы: -1 = текущий процесс (GetCurrentProcess)

/// Дескриптор объекта. Непрозрачный для приложения.
///
/// Размер: pointer-sized (4 байта на x86, 8 байт на x64).
/// Кратен 4 (младшие 2 бита зарезервированы ядром).
///
/// Примеры объектов, к которым даёт доступ HANDLE:
///   - File (файл / пайп / устройство)
///   - Process / Thread
///   - Mutex / Semaphore / Event
///   - Registry Key
///   - Section (memory-mapped file)
///   - Token (security)
///   - Window Station / Desktop
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HANDLE(pub usize);

impl HANDLE {
    pub const NULL: HANDLE = HANDLE(0);
    pub const INVALID: HANDLE = HANDLE(usize::MAX); // INVALID_HANDLE_VALUE

    pub fn is_null(self) -> bool { self.0 == 0 }
    pub fn is_invalid(self) -> bool { self.0 == usize::MAX }
    pub fn is_valid(self) -> bool { !self.is_null() && !self.is_invalid() }
    pub fn as_usize(self) -> usize { self.0 }
}

/// INVALID_HANDLE_VALUE — возвращается при ошибке CreateFile и подобными.
pub const INVALID_HANDLE_VALUE: HANDLE = HANDLE::INVALID;

// ---- NTSTATUS ----
//
// Формат кода статуса NT (32 бита):
//
//   ┌──┬──┬──────────────┬────────────────────────────┐
//   │SS│C │  Facility    │      Code                  │
//   │2b│1b│  13 bits     │      16 bits               │
//   └──┴──┴──────────────┴────────────────────────────┘
//
//   SS (Severity, биты 30-31):
//     00 = Success
//     01 = Informational
//     10 = Warning
//     11 = Error
//
//   C (Customer, бит 29):
//     0 = системный код (определён Microsoft)
//     1 = пользовательский код
//
//   Facility (биты 16-28):
//     Подсистема, которая сгенерировала код.
//     0 = общие, 1 = RPC, 2 = Dispatcher, ...
//
//   Code (биты 0-15):
//     Конкретный код ошибки внутри facility.
//
// Примеры:
//   STATUS_SUCCESS         = 0x00000000 (SS=00, всё OK)
//   STATUS_ACCESS_DENIED   = 0xC0000022 (SS=11, ошибка)
//   STATUS_NO_MEMORY       = 0xC0000017 (SS=11, ошибка)
//   STATUS_PENDING         = 0x00000103 (SS=00, информация)
//
// Win32 error codes (GetLastError) — ДРУГАЯ система!
// NtStatusToWin32Error() конвертирует NT → Win32.
// Наш error.rs содержит обе системы и маппинг.

/// NT Status Code — результат любой NT API функции.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct NTSTATUS(pub u32);

impl NTSTATUS {
    /// Успех? (биты 30-31 = 00)
    pub fn is_success(self) -> bool { self.0 < 0x80000000 }

    /// Ошибка? (бит 31 = 1)
    pub fn is_error(self) -> bool { self.0 >= 0xC0000000 }

    /// Предупреждение? (биты 30-31 = 10)
    pub fn is_warning(self) -> bool { self.0 >= 0x80000000 && self.0 < 0xC0000000 }

    /// Информация? (биты 30-31 = 01)
    pub fn is_info(self) -> bool { self.0 >= 0x40000000 && self.0 < 0x80000000 }

    /// Facility (подсистема).
    pub fn facility(self) -> u16 { ((self.0 >> 16) & 0x1FFF) as u16 }

    /// Code (конкретная ошибка).
    pub fn code(self) -> u16 { (self.0 & 0xFFFF) as u16 }

    /// Сырое значение.
    pub fn raw(self) -> u32 { self.0 }
}

impl core::fmt::Debug for NTSTATUS {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let severity = match self.0 >> 30 {
            0 => "SUCCESS",
            1 => "INFO",
            2 => "WARNING",
            3 => "ERROR",
            _ => "?",
        };
        write!(f, "NTSTATUS({:#010x} [{}])", self.0, severity)
    }
}

// ---- UNICODE_STRING ----
//
// Windows ВНУТРЕННЕ НЕ использует null-terminated строки!
// Вместо этого — UNICODE_STRING: указатель + длина.
//
// Это безопаснее (нет buffer overrun при поиске нуля)
// и быстрее (длина известна заранее, не нужен strlen).
//
// Все NT API принимают UNICODE_STRING.
// Win32 API (CreateFileW) принимает LPCWSTR (null-terminated),
// но внутри конвертирует в UNICODE_STRING перед вызовом NtCreateFile.
//
// ReactOS: sdk/include/ndk/umtypes.h
// Wine: include/winternl.h

/// Счётная Unicode-строка (UTF-16).
///
/// `length` — длина строки В БАЙТАХ (не символах!).
/// `maximum_length` — размер буфера В БАЙТАХ.
/// `buffer` — указатель на UTF-16 данные.
///
/// Пример: строка "Hello" (5 символов, 10 байт):
///   length = 10, maximum_length = 12 (с запасом), buffer → [H,e,l,l,o]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UNICODE_STRING {
    pub length: USHORT,          // Длина в байтах (без null-terminator)
    pub maximum_length: USHORT,  // Размер буфера в байтах
    pub buffer: *mut WCHAR,      // Указатель на UTF-16 данные
}

impl UNICODE_STRING {
    pub const fn empty() -> Self {
        UNICODE_STRING {
            length: 0,
            maximum_length: 0,
            buffer: core::ptr::null_mut(),
        }
    }

    /// Количество символов (code units) в строке.
    pub fn char_count(&self) -> usize {
        self.length as usize / 2
    }

    /// Строка пуста?
    pub fn is_empty(&self) -> bool {
        self.length == 0 || self.buffer.is_null()
    }
}

impl core::fmt::Debug for UNICODE_STRING {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "UNICODE_STRING(len={}, max={})", self.length, self.maximum_length)
    }
}

/// ANSI строка (аналогично, но 8-bit).
/// Используется только для обратной совместимости с Win9x API.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ANSI_STRING {
    pub length: USHORT,
    pub maximum_length: USHORT,
    pub buffer: *mut CHAR,
}

// ---- OBJECT_ATTRIBUTES ----
//
// Структура, передаваемая в NtCreateFile, NtOpenKey и другие NT API.
// Описывает «какой объект открыть и с какими параметрами».
//
// ReactOS: sdk/include/ndk/obtypes.h
// Wine: include/winternl.h

/// Атрибуты объекта для NT API.
///
/// Пример использования:
/// ```
/// OBJECT_ATTRIBUTES oa;
/// InitializeObjectAttributes(&oa, &name, OBJ_CASE_INSENSITIVE, NULL, NULL);
/// NtCreateFile(&handle, GENERIC_READ, &oa, &iosb, ...);
/// ```
#[repr(C)]
pub struct OBJECT_ATTRIBUTES {
    /// Размер структуры (для версионирования).
    pub length: ULONG,

    /// Корневой каталог (HANDLE). Если NULL — имя абсолютное.
    /// Если задан — имя относительно этого каталога.
    pub root_directory: HANDLE,

    /// Имя объекта (путь в Object Namespace).
    /// Пример: "\Device\HarddiskVolume1\Windows\notepad.exe"
    pub object_name: *const UNICODE_STRING,

    /// Флаги:
    ///   OBJ_CASE_INSENSITIVE = 0x40 — без учёта регистра
    ///   OBJ_INHERIT          = 0x02 — дочерние процессы наследуют хэндл
    ///   OBJ_PERMANENT        = 0x10 — объект не удаляется при закрытии последнего хэндла
    pub attributes: ULONG,

    /// Security Descriptor (NULL = наследуемый по умолчанию).
    pub security_descriptor: *const u8,

    /// Security Quality of Service (для impersonation).
    pub security_qos: *const u8,
}

impl OBJECT_ATTRIBUTES {
    pub const fn empty() -> Self {
        OBJECT_ATTRIBUTES {
            length: core::mem::size_of::<OBJECT_ATTRIBUTES>() as ULONG,
            root_directory: HANDLE::NULL,
            object_name: core::ptr::null(),
            attributes: 0,
            security_descriptor: core::ptr::null(),
            security_qos: core::ptr::null(),
        }
    }
}

// Флаги OBJECT_ATTRIBUTES
pub const OBJ_INHERIT: ULONG            = 0x00000002;
pub const OBJ_PERMANENT: ULONG          = 0x00000010;
pub const OBJ_EXCLUSIVE: ULONG          = 0x00000020;
pub const OBJ_CASE_INSENSITIVE: ULONG   = 0x00000040;
pub const OBJ_OPENIF: ULONG             = 0x00000080;
pub const OBJ_OPENLINK: ULONG           = 0x00000100;
pub const OBJ_KERNEL_HANDLE: ULONG      = 0x00000200;

// ---- IO_STATUS_BLOCK ----
//
// Возвращается из NtCreateFile, NtReadFile, NtWriteFile и т.д.
// Содержит результат операции и количество переданных байт.

/// Результат I/O операции.
#[repr(C)]
pub struct IO_STATUS_BLOCK {
    /// Статус операции (NTSTATUS).
    pub status: NTSTATUS,

    /// Количество переданных байт / дополнительная информация.
    pub information: ULONG_PTR,
}

impl IO_STATUS_BLOCK {
    pub const fn empty() -> Self {
        IO_STATUS_BLOCK {
            status: NTSTATUS(0),
            information: 0,
        }
    }
}

// ---- LARGE_INTEGER ----
//
// 64-битное целое, используемое в старых API (до C99 int64_t).
// Windows определяет его как union { struct { DWORD Low; LONG High; }; LONGLONG QuadPart; }
// Мы упрощаем до i64.

/// 64-bit signed integer (аналог LARGE_INTEGER.QuadPart).
pub type LARGE_INTEGER = i64;

/// 64-bit unsigned.
pub type ULARGE_INTEGER = u64;

// ---- GUID ----
//
// Globally Unique Identifier — 128-битный идентификатор.
// Используется ВЕЗДЕ в Windows: COM, DirectX, реестр, WMI, устройства.
// Формат: {XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}
// Пример: {6B29FC40-CA47-1067-B31D-00DD010662DA}

/// 128-битный глобально уникальный идентификатор.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GUID {
    pub data1: DWORD,    // 32 бита
    pub data2: WORD,     // 16 бит
    pub data3: WORD,     // 16 бит
    pub data4: [BYTE; 8], // 64 бита
}

impl GUID {
    pub const EMPTY: GUID = GUID {
        data1: 0, data2: 0, data3: 0, data4: [0; 8],
    };
}

// ---- FILETIME ----
//
// Время в Windows — количество 100-наносекундных интервалов
// с 1 января 1601 года (начало григорианского календаря).
//
// Почему 1601? Потому что это начало 400-летнего цикла
// григорианского календаря, что упрощает вычисления високосных годов.

/// Время файла (100-ns интервалы с 01.01.1601).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FILETIME {
    pub low_date_time: DWORD,
    pub high_date_time: DWORD,
}

impl FILETIME {
    /// Получить как 64-битное значение.
    pub fn as_u64(&self) -> u64 {
        (self.high_date_time as u64) << 32 | self.low_date_time as u64
    }
}

// ---- LIST_ENTRY ----
//
// Двусвязный список — ОСНОВНАЯ структура данных в NT ядре.
// Используется повсюду: список процессов, потоков, объектов, IRQ, DPC.
//
// NT не использует отдельные узлы — LIST_ENTRY встраивается ВНУТРЬ структуры:
//
//   struct Process {
//       LIST_ENTRY process_links;   // ← связь с другими процессами
//       DWORD pid;
//       ...
//   };
//
// Голова списка — тоже LIST_ENTRY. Пустой список: Flink = Blink = &Head.
//
// Макрос CONTAINING_RECORD() получает указатель на структуру из указателя на LIST_ENTRY.
// Это аналог Linux container_of().

/// Элемент двусвязного списка NT.
///
/// Встраивается в структуры как поле.
/// `flink` — Forward Link (следующий элемент).
/// `blink` — Backward Link (предыдущий элемент).
#[repr(C)]
pub struct LIST_ENTRY {
    pub flink: *mut LIST_ENTRY,
    pub blink: *mut LIST_ENTRY,
}

impl LIST_ENTRY {
    /// Инициализировать как пустой список (голова указывает на себя).
    pub fn init_head(head: *mut LIST_ENTRY) {
        unsafe {
            (*head).flink = head;
            (*head).blink = head;
        }
    }

    /// Список пуст? (голова указывает на себя)
    pub fn is_empty(head: *const LIST_ENTRY) -> bool {
        unsafe { (*head).flink as *const _ == head }
    }
}

// ---- ACCESS_MASK ----
//
// Битовая маска прав доступа. Передаётся в NtCreateFile, NtOpenKey и т.д.
//
// Формат (32 бита):
//   биты 0-15:  Specific Rights (зависят от типа объекта)
//   биты 16-23: Standard Rights (общие для всех объектов)
//   биты 24-27: Reserved
//   бит 28:     GENERIC_ALL
//   бит 29:     GENERIC_EXECUTE
//   бит 30:     GENERIC_WRITE
//   бит 31:     GENERIC_READ

/// Маска прав доступа к объекту.
pub type ACCESS_MASK = DWORD;

// Стандартные права
pub const DELETE: ACCESS_MASK                  = 0x00010000;
pub const READ_CONTROL: ACCESS_MASK            = 0x00020000;
pub const WRITE_DAC: ACCESS_MASK               = 0x00040000;
pub const WRITE_OWNER: ACCESS_MASK             = 0x00080000;
pub const SYNCHRONIZE: ACCESS_MASK             = 0x00100000;

// Общие права (маппятся на конкретные в зависимости от типа объекта)
pub const GENERIC_READ: ACCESS_MASK            = 0x80000000;
pub const GENERIC_WRITE: ACCESS_MASK           = 0x40000000;
pub const GENERIC_EXECUTE: ACCESS_MASK         = 0x20000000;
pub const GENERIC_ALL: ACCESS_MASK             = 0x10000000;

// Файловые права
pub const FILE_READ_DATA: ACCESS_MASK          = 0x00000001;
pub const FILE_WRITE_DATA: ACCESS_MASK         = 0x00000002;
pub const FILE_APPEND_DATA: ACCESS_MASK        = 0x00000004;
pub const FILE_READ_EA: ACCESS_MASK            = 0x00000008;
pub const FILE_WRITE_EA: ACCESS_MASK           = 0x00000010;
pub const FILE_EXECUTE: ACCESS_MASK            = 0x00000020;
pub const FILE_READ_ATTRIBUTES: ACCESS_MASK    = 0x00000080;
pub const FILE_WRITE_ATTRIBUTES: ACCESS_MASK   = 0x00000100;

// Права процесса
pub const PROCESS_TERMINATE: ACCESS_MASK       = 0x00000001;
pub const PROCESS_CREATE_THREAD: ACCESS_MASK   = 0x00000002;
pub const PROCESS_VM_OPERATION: ACCESS_MASK    = 0x00000008;
pub const PROCESS_VM_READ: ACCESS_MASK         = 0x00000010;
pub const PROCESS_VM_WRITE: ACCESS_MASK        = 0x00000020;
pub const PROCESS_DUP_HANDLE: ACCESS_MASK      = 0x00000040;
pub const PROCESS_CREATE_PROCESS: ACCESS_MASK  = 0x00000080;
pub const PROCESS_QUERY_INFORMATION: ACCESS_MASK = 0x00000400;
pub const PROCESS_ALL_ACCESS: ACCESS_MASK      = 0x001FFFFF;

// ---- Типы указателей ----
//
// В Win32 API указатели имеют специальные typedef-ы:
//   LPVOID  = void*          (любой указатель)
//   LPCVOID = const void*    (указатель на read-only данные)
//   LPSTR   = char*          (ANSI строка)
//   LPCSTR  = const char*    (read-only ANSI строка)
//   LPWSTR  = WCHAR*         (Wide строка)
//   LPCWSTR = const WCHAR*   (read-only Wide строка)
//
// LP = "Long Pointer" — наследие 16-bit Windows (far pointer).
// Сейчас LP = P = обычный указатель.
//
// В Rust мы используем *mut/*const, но определяем type aliases
// для совместимости с документацией и API сигнатурами.

pub type PVOID = *mut u8;
pub type LPVOID = *mut u8;
pub type LPCVOID = *const u8;

pub type LPSTR = *mut u8;
pub type LPCSTR = *const u8;

pub type LPWSTR = *mut WCHAR;
pub type LPCWSTR = *const WCHAR;

pub type PDWORD = *mut DWORD;
pub type PHANDLE = *mut HANDLE;

// ---- CLIENT_ID ----
//
// Идентификатор процесса + потока. Передаётся в NtCreateThread и т.д.

/// Пара PID + TID.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CLIENT_ID {
    pub unique_process: HANDLE,  // PID как HANDLE
    pub unique_thread: HANDLE,   // TID как HANDLE
}

// ---- Вспомогательные макросы/функции ----

/// NT_SUCCESS() — проверка успешности NTSTATUS (аналог макроса из Windows DDK).
pub fn nt_success(status: NTSTATUS) -> bool {
    status.is_success()
}

/// LOWORD/HIWORD — извлечение 16-битных половинок из 32-битного значения.
/// Используется постоянно в Win32 для упаковки двух значений в одно DWORD.
/// Пример: lParam оконного сообщения = MAKELONG(x, y).
pub fn loword(dw: DWORD) -> WORD { dw as WORD }
pub fn hiword(dw: DWORD) -> WORD { (dw >> 16) as WORD }
pub fn lobyte(w: WORD) -> BYTE { w as BYTE }
pub fn hibyte(w: WORD) -> BYTE { (w >> 8) as BYTE }
pub fn makelong(low: WORD, high: WORD) -> DWORD {
    (low as DWORD) | ((high as DWORD) << 16)
}
pub fn makeword(low: BYTE, high: BYTE) -> WORD {
    (low as WORD) | ((high as WORD) << 8)
}
