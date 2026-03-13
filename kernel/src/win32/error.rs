// =============================================================================
// NoNameOS — NTSTATUS + Win32 Error Codes
// =============================================================================
//
// В Windows ДВЕ системы кодов ошибок:
//
//   1. NTSTATUS — используется NT Native API (NtCreateFile и т.д.)
//      32-битный код с severity, facility, code.
//      Определены в ntstatus.h
//
//   2. Win32 Error Codes — используется Win32 API (CreateFile и т.д.)
//      32-битное число. Получается через GetLastError()
//      Определены в winerror.h
//
// Связь между ними:
//   KERNEL32.CreateFile() → вызывает NTDLL.NtCreateFile()
//   NtCreateFile() возвращает NTSTATUS
//   KERNEL32 конвертирует NTSTATUS → Win32 Error Code через RtlNtStatusToDosError()
//   Сохраняет в TEB.LastErrorValue
//   Приложение читает через GetLastError()
//
// Нам нужны ОБЕ системы:
//   - NTSTATUS — для внутренних API ядра
//   - Win32 Errors — для совместимости с приложениями
//
// Источники:
//   - Wine: include/ntstatus.h, include/winerror.h, dlls/ntdll/error.c
//   - ReactOS: sdk/include/ndk/ntstatus.h, sdk/include/xdk/winerror.h
//   - Microsoft DDK: ntstatus.h
// =============================================================================

use super::types::{NTSTATUS, DWORD};

// =============================================================================
// NTSTATUS — коды статуса NT ядра
// =============================================================================
//
// Мы определяем только самые важные, с которых можно начать.
// Полный список: ~2500 значений. Добавится по факту.
//
// Конвенция именования: STATUS_*

// ---- Success (0x0000xxxx) ----
pub const STATUS_SUCCESS: NTSTATUS                  = NTSTATUS(0x00000000);
pub const STATUS_PENDING: NTSTATUS                  = NTSTATUS(0x00000103);
pub const STATUS_REPARSE: NTSTATUS                  = NTSTATUS(0x00000104);
pub const STATUS_MORE_ENTRIES: NTSTATUS             = NTSTATUS(0x00000105);
pub const STATUS_BUFFER_OVERFLOW: NTSTATUS          = NTSTATUS(0x80000005);

// ---- Informational (0x4000xxxx) ----
pub const STATUS_OBJECT_NAME_EXISTS: NTSTATUS       = NTSTATUS(0x40000000);

// ---- Warning (0x8000xxxx) ----
pub const STATUS_BUFFER_TOO_SMALL: NTSTATUS         = NTSTATUS(0x80000023);

// ---- Error (0xC000xxxx) ----

/// Недостаточно прав.
pub const STATUS_ACCESS_DENIED: NTSTATUS            = NTSTATUS(0xC0000022);

/// Неверный параметр функции.
pub const STATUS_INVALID_PARAMETER: NTSTATUS        = NTSTATUS(0xC000000D);

/// Недостаточно памяти.
pub const STATUS_NO_MEMORY: NTSTATUS                = NTSTATUS(0xC0000017);
pub const STATUS_INSUFFICIENT_RESOURCES: NTSTATUS   = NTSTATUS(0xC000009A);

/// Объект не найден (файл, ключ реестра, ...).
pub const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS    = NTSTATUS(0xC0000034);
pub const STATUS_OBJECT_NAME_COLLISION: NTSTATUS    = NTSTATUS(0xC0000035);
pub const STATUS_OBJECT_PATH_NOT_FOUND: NTSTATUS    = NTSTATUS(0xC000003A);
pub const STATUS_OBJECT_PATH_SYNTAX_BAD: NTSTATUS   = NTSTATUS(0xC000003B);

/// Неверный хэндл.
pub const STATUS_INVALID_HANDLE: NTSTATUS           = NTSTATUS(0xC0000008);
pub const STATUS_OBJECT_TYPE_MISMATCH: NTSTATUS     = NTSTATUS(0xC0000024);

/// Операция не поддерживается.
pub const STATUS_NOT_IMPLEMENTED: NTSTATUS          = NTSTATUS(0xC0000002);
pub const STATUS_NOT_SUPPORTED: NTSTATUS            = NTSTATUS(0xC00000BB);
pub const STATUS_ILLEGAL_FUNCTION: NTSTATUS         = NTSTATUS(0xC00000AF);

/// Буфер слишком мал.
pub const STATUS_INFO_LENGTH_MISMATCH: NTSTATUS     = NTSTATUS(0xC0000004);

/// Нарушение доступа к памяти (аналог SIGSEGV).
pub const STATUS_ACCESS_VIOLATION: NTSTATUS         = NTSTATUS(0xC0000005);

/// Неверная инструкция.
pub const STATUS_ILLEGAL_INSTRUCTION: NTSTATUS      = NTSTATUS(0xC000001D);

/// Деление на ноль.
pub const STATUS_INTEGER_DIVIDE_BY_ZERO: NTSTATUS   = NTSTATUS(0xC0000094);
pub const STATUS_INTEGER_OVERFLOW: NTSTATUS         = NTSTATUS(0xC0000095);

/// Конец файла.
pub const STATUS_END_OF_FILE: NTSTATUS              = NTSTATUS(0xC0000011);

/// Файловая система.
pub const STATUS_NO_SUCH_FILE: NTSTATUS             = NTSTATUS(0xC000000F);
pub const STATUS_FILE_IS_A_DIRECTORY: NTSTATUS      = NTSTATUS(0xC00000BA);
pub const STATUS_NOT_A_DIRECTORY: NTSTATUS          = NTSTATUS(0xC0000103);
pub const STATUS_DIRECTORY_NOT_EMPTY: NTSTATUS      = NTSTATUS(0xC0000101);
pub const STATUS_SHARING_VIOLATION: NTSTATUS        = NTSTATUS(0xC0000043);
pub const STATUS_DELETE_PENDING: NTSTATUS           = NTSTATUS(0xC0000056);
pub const STATUS_OBJECT_NAME_INVALID: NTSTATUS      = NTSTATUS(0xC0000033);

/// Процессы и потоки.
pub const STATUS_PROCESS_IS_TERMINATING: NTSTATUS   = NTSTATUS(0xC000010A);
pub const STATUS_THREAD_IS_TERMINATING: NTSTATUS    = NTSTATUS(0xC000004B);

/// Виртуальная память.
pub const STATUS_CONFLICTING_ADDRESSES: NTSTATUS    = NTSTATUS(0xC0000018);
pub const STATUS_UNABLE_TO_FREE_VM: NTSTATUS        = NTSTATUS(0xC000001A);
pub const STATUS_SECTION_NOT_IMAGE: NTSTATUS        = NTSTATUS(0xC0000049);
pub const STATUS_INVALID_IMAGE_FORMAT: NTSTATUS     = NTSTATUS(0xC000007B);

/// PE загрузчик.
pub const STATUS_INVALID_IMAGE_NOT_MZ: NTSTATUS     = NTSTATUS(0xC000012F);
pub const STATUS_IMAGE_MACHINE_TYPE_MISMATCH: NTSTATUS = NTSTATUS(0x4000000E);

/// Реестр.
pub const STATUS_KEY_DELETED: NTSTATUS              = NTSTATUS(0xC000017C);
pub const STATUS_NO_MORE_ENTRIES: NTSTATUS          = NTSTATUS(0x8000001A);

// Это ДРУГАЯ система нумерации (не NTSTATUS!).
// Приложения видят именно эти коды.
// Полный список: около 15000 значений.

pub const ERROR_SUCCESS: DWORD              = 0;
pub const ERROR_INVALID_FUNCTION: DWORD     = 1;
pub const ERROR_FILE_NOT_FOUND: DWORD       = 2;
pub const ERROR_PATH_NOT_FOUND: DWORD       = 3;
pub const ERROR_TOO_MANY_OPEN_FILES: DWORD  = 4;
pub const ERROR_ACCESS_DENIED: DWORD        = 5;
pub const ERROR_INVALID_HANDLE: DWORD       = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: DWORD    = 8;
pub const ERROR_OUTOFMEMORY: DWORD          = 14;
pub const ERROR_INVALID_DRIVE: DWORD        = 15;
pub const ERROR_NO_MORE_FILES: DWORD        = 18;
pub const ERROR_WRITE_PROTECT: DWORD        = 19;
pub const ERROR_NOT_READY: DWORD            = 21;
pub const ERROR_SHARING_VIOLATION: DWORD    = 32;
pub const ERROR_FILE_EXISTS: DWORD          = 80;
pub const ERROR_INVALID_PARAMETER: DWORD    = 87;
pub const ERROR_BROKEN_PIPE: DWORD          = 109;
pub const ERROR_INSUFFICIENT_BUFFER: DWORD  = 122;
pub const ERROR_INVALID_NAME: DWORD         = 123;
pub const ERROR_MOD_NOT_FOUND: DWORD        = 126;
pub const ERROR_PROC_NOT_FOUND: DWORD       = 127;
pub const ERROR_DIR_NOT_EMPTY: DWORD        = 145;
pub const ERROR_ALREADY_EXISTS: DWORD       = 183;
pub const ERROR_ENVVAR_NOT_FOUND: DWORD     = 203;
pub const ERROR_MORE_DATA: DWORD            = 234;
pub const ERROR_NO_MORE_ITEMS: DWORD        = 259;
pub const ERROR_DIRECTORY: DWORD            = 267;
pub const ERROR_NOT_FOUND: DWORD            = 1168;
pub const ERROR_IO_PENDING: DWORD           = 997;
pub const ERROR_NOACCESS: DWORD             = 998;
pub const ERROR_NOT_SUPPORTED: DWORD        = 50;

// Конвертация NTSTATUS → Win32 Error Code
//
// Когда Win32 API (например CreateFile) вызывает NtCreateFile(),
// возвращённый NTSTATUS нужно преобразовать в Win32 error code
// для SetLastError()/GetLastError().
//
// В Windows это делает RtlNtStatusToDosError() (ntdll.dll).
// В Wine: dlls/ntdll/error.c — огромная таблица маппинга.
// В ReactOS: ntoskrnl/rtl/error.c.
//
// Пока реализуем маппинг для самых частых кодов.
// По мере добавления API — расширяем таблицу.

/// Преобразование NTSTATUS → Win32 Error Code.
///
/// Аналог RtlNtStatusToDosError() из NTDLL.
/// Если код неизвестен — возвращает ERROR_MR_MID_NOT_FOUND (317).
pub fn ntstatus_to_win32_error(status: NTSTATUS) -> DWORD {
    match status {
        // Success
        STATUS_SUCCESS                  => ERROR_SUCCESS,
        STATUS_PENDING                  => ERROR_IO_PENDING,
        STATUS_BUFFER_OVERFLOW          => ERROR_MORE_DATA,

        // File/Object not found
        STATUS_OBJECT_NAME_NOT_FOUND    => ERROR_FILE_NOT_FOUND,
        STATUS_OBJECT_PATH_NOT_FOUND    => ERROR_PATH_NOT_FOUND,
        STATUS_NO_SUCH_FILE             => ERROR_FILE_NOT_FOUND,

        // Access
        STATUS_ACCESS_DENIED            => ERROR_ACCESS_DENIED,
        STATUS_ACCESS_VIOLATION         => ERROR_NOACCESS,

        // Handle
        STATUS_INVALID_HANDLE           => ERROR_INVALID_HANDLE,

        // Memory
        STATUS_NO_MEMORY                => ERROR_NOT_ENOUGH_MEMORY,
        STATUS_INSUFFICIENT_RESOURCES   => ERROR_OUTOFMEMORY,

        // Parameters
        STATUS_INVALID_PARAMETER        => ERROR_INVALID_PARAMETER,

        // File system
        STATUS_OBJECT_NAME_COLLISION    => ERROR_ALREADY_EXISTS,
        STATUS_SHARING_VIOLATION        => ERROR_SHARING_VIOLATION,
        STATUS_DIRECTORY_NOT_EMPTY      => ERROR_DIR_NOT_EMPTY,
        STATUS_FILE_IS_A_DIRECTORY      => ERROR_DIRECTORY,
        STATUS_OBJECT_NAME_INVALID      => ERROR_INVALID_NAME,
        STATUS_END_OF_FILE              => ERROR_NO_MORE_FILES,

        // Not implemented
        STATUS_NOT_IMPLEMENTED          => ERROR_INVALID_FUNCTION,
        STATUS_NOT_SUPPORTED            => ERROR_NOT_SUPPORTED,

        // Buffer
        STATUS_BUFFER_TOO_SMALL         => ERROR_INSUFFICIENT_BUFFER,
        STATUS_INFO_LENGTH_MISMATCH     => ERROR_INSUFFICIENT_BUFFER,

        // Image
        STATUS_INVALID_IMAGE_FORMAT     => ERROR_MOD_NOT_FOUND,

        // Всё остальное — generic
        _ => {
            if status.is_success() {
                ERROR_SUCCESS
            } else {
                317 // ERROR_MR_MID_NOT_FOUND — «неизвестный код»
            }
        }
    }
}

/// Обратное преобразование Win32 Error → NTSTATUS.
/// Нужно для некоторых API.
pub fn win32_error_to_ntstatus(error: DWORD) -> NTSTATUS {
    match error {
        ERROR_SUCCESS              => STATUS_SUCCESS,
        ERROR_FILE_NOT_FOUND       => STATUS_OBJECT_NAME_NOT_FOUND,
        ERROR_PATH_NOT_FOUND       => STATUS_OBJECT_PATH_NOT_FOUND,
        ERROR_ACCESS_DENIED        => STATUS_ACCESS_DENIED,
        ERROR_INVALID_HANDLE       => STATUS_INVALID_HANDLE,
        ERROR_NOT_ENOUGH_MEMORY    => STATUS_NO_MEMORY,
        ERROR_INVALID_PARAMETER    => STATUS_INVALID_PARAMETER,
        ERROR_ALREADY_EXISTS       => STATUS_OBJECT_NAME_COLLISION,
        ERROR_SHARING_VIOLATION    => STATUS_SHARING_VIOLATION,
        ERROR_INSUFFICIENT_BUFFER  => STATUS_BUFFER_TOO_SMALL,
        ERROR_NOT_SUPPORTED        => STATUS_NOT_SUPPORTED,
        _                          => NTSTATUS(0xC0000001), // STATUS_UNSUCCESSFUL
    }
}

/// Имя NTSTATUS для отладочного вывода.
pub fn ntstatus_name(status: NTSTATUS) -> &'static str {
    match status {
        STATUS_SUCCESS                  => "STATUS_SUCCESS",
        STATUS_PENDING                  => "STATUS_PENDING",
        STATUS_ACCESS_DENIED            => "STATUS_ACCESS_DENIED",
        STATUS_INVALID_PARAMETER        => "STATUS_INVALID_PARAMETER",
        STATUS_NO_MEMORY                => "STATUS_NO_MEMORY",
        STATUS_OBJECT_NAME_NOT_FOUND    => "STATUS_OBJECT_NAME_NOT_FOUND",
        STATUS_OBJECT_PATH_NOT_FOUND    => "STATUS_OBJECT_PATH_NOT_FOUND",
        STATUS_INVALID_HANDLE           => "STATUS_INVALID_HANDLE",
        STATUS_NOT_IMPLEMENTED          => "STATUS_NOT_IMPLEMENTED",
        STATUS_ACCESS_VIOLATION         => "STATUS_ACCESS_VIOLATION",
        STATUS_INVALID_IMAGE_FORMAT     => "STATUS_INVALID_IMAGE_FORMAT",
        STATUS_SHARING_VIOLATION        => "STATUS_SHARING_VIOLATION",
        STATUS_END_OF_FILE              => "STATUS_END_OF_FILE",
        STATUS_BUFFER_OVERFLOW          => "STATUS_BUFFER_OVERFLOW",
        STATUS_BUFFER_TOO_SMALL         => "STATUS_BUFFER_TOO_SMALL",
        _                               => "STATUS_UNKNOWN",
    }
}
