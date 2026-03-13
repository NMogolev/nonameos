// =============================================================================
// NoNameOS — PE Format Parser
// =============================================================================
//
// PE — формат исполняемых файлов Windows (.exe, .dll, .sys, .ocx).
// Чтобы запустить любую Windows-программу, нужно уметь его парсить.
//
// Структура PE файла:
//
//   ┌──────────────────────────────┐  Offset 0
//   │ DOS Header (64 bytes)        │  ← "MZ" magic (0x5A4D)
//   │   e_lfanew → PE Header       │  ← смещение до PE Header
//   ├──────────────────────────────┤
//   │ DOS Stub (переменный размер) │  ← "This program cannot be run in DOS mode"
//   ├──────────────────────────────┤  Offset = e_lfanew
//   │ PE Signature (4 bytes)       │  ← "PE\0\0" (0x00004550)
//   ├──────────────────────────────┤
//   │ COFF Header (20 bytes)       │  ← Machine, NumberOfSections, TimeDateStamp
//   ├──────────────────────────────┤
//   │ Optional Header              │  ← AddressOfEntryPoint, ImageBase, DataDirectory
//   │   PE32:  224 bytes           │     (32-bit)
//   │   PE32+: 240 bytes           │     (64-bit) ← нам нужен этот
//   ├──────────────────────────────┤
//   │ Section Headers              │  ← .text, .data, .rdata, .rsrc, .reloc
//   │   (40 bytes × N sections)    │
//   ├──────────────────────────────┤
//   │ Section Data                 │  ← код, данные, ресурсы, импорты
//   │   .text  — исполняемый код   │
//   │   .data  — инициализированные│
//   │   .rdata — read-only данные  │
//   │   .idata — таблица импортов  │
//   │   .rsrc  — ресурсы (иконки)  │
//   │   .reloc — relocations       │
//   └──────────────────────────────┘
//
// Загрузка PE файла (что делает CreateProcess → NtCreateSection → MmMapViewOfSection):
//
//   1. Прочитать DOS Header, проверить "MZ"
//   2. Перейти к PE Header (по e_lfanew), проверить "PE\0\0"
//   3. Прочитать COFF Header — узнать архитектуру и кол-во секций
//   4. Прочитать Optional Header — узнать entry point, image base, data directories
//   5. Замапить секции в память по их VirtualAddress
//   6. Обработать relocations (если ImageBase занят)
//   7. Обработать Import Table — загрузить зависимые DLL
//   8. Вызвать AddressOfEntryPoint
//
// Import Table (таблица импортов):
//   Содержит список DLL и функций, которые нужны программе.
//   Пример: notepad.exe импортирует kernel32.dll!CreateFileW
//
//   ┌─────────────────────────────┐
//   │ Import Directory Table      │  ← массив IMAGE_IMPORT_DESCRIPTOR
//   │   Name: "kernel32.dll"      │
//   │   OriginalFirstThunk → IAT  │
//   │   Name: "user32.dll"        │
//   │   OriginalFirstThunk → IAT  │
//   │   (null terminator)         │
//   ├─────────────────────────────┤
//   │ Import Address Table (IAT)  │  ← адреса функций (заполняются при загрузке)
//   │   [0] → CreateFileW         │
//   │   [1] → ReadFile            │
//   │   [2] → WriteFile           │
//   └─────────────────────────────┘
//
// При загрузке загрузчик заменяет записи IAT реальными адресами функций.
// Это ключевой момент для совместимости: мы подставляем НАШИ реализации!
//
// Export Table (таблица экспортов):
//   В DLL — список функций, доступных для других модулей.
//   kernel32.dll экспортирует CreateFileW, ReadFile и т.д.
//
// Источники:
//   - Microsoft PE/COFF Specification (MS docs)
//   - Wine: dlls/ntdll/loader.c, include/winnt.h (IMAGE_* структуры)
//   - ReactOS: ntoskrnl/mm/section.c, sdk/include/reactos/wine/winnt.h
//   - https://docs.microsoft.com/en-us/windows/win32/debug/pe-format
// =============================================================================

use super::types::*;

// ---- DOS Header ----
//
// Самые первые байты любого PE файла.
// Наследие MS-DOS (1981 год!). Нужен только для поля e_lfanew.

/// DOS Header — первые 64 байта PE файла.
///
/// Единственные важные поля:
///   `e_magic` — должно быть 0x5A4D ("MZ", Mark Zbikowski — разработчик MS-DOS)
///   `e_lfanew` — смещение PE-заголовка от начала файла
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageDosHeader {
    pub e_magic: WORD,       // "MZ" = 0x5A4D
    pub e_cblp: WORD,
    pub e_cp: WORD,
    pub e_crlc: WORD,
    pub e_cparhdr: WORD,
    pub e_minalloc: WORD,
    pub e_maxalloc: WORD,
    pub e_ss: WORD,
    pub e_sp: WORD,
    pub e_csum: WORD,
    pub e_ip: WORD,
    pub e_cs: WORD,
    pub e_lfarlc: WORD,
    pub e_ovno: WORD,
    pub e_res: [WORD; 4],
    pub e_oemid: WORD,
    pub e_oeminfo: WORD,
    pub e_res2: [WORD; 10],
    pub e_lfanew: LONG,
}

/// Магическое число DOS Header.
pub const IMAGE_DOS_SIGNATURE: WORD = 0x5A4D; // "MZ"

// ---- PE Signature ----

/// Магическое число PE Header.
pub const IMAGE_NT_SIGNATURE: DWORD = 0x00004550; // "PE\0\0"

// ---- COFF Header (File Header) ----
//
// Описывает общие свойства файла: архитектуру, количество секций, timestamp.

/// COFF File Header — 20 байт после PE signature.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageFileHeader {
    /// Целевая архитектура.
    ///   0x014C = i386 (32-bit)
    ///   0x8664 = AMD64 (64-bit)
    ///   0xAA64 = ARM64
    pub machine: WORD,

    /// Количество секций (.text, .data, .rdata...).
    pub number_of_sections: WORD,

    /// Unix timestamp создания файла (секунды с 01.01.1970).
    pub time_date_stamp: DWORD,

    /// Указатель на таблицу символов (обычно 0, отладочная информация).
    pub pointer_to_symbol_table: DWORD,

    /// Количество символов.
    pub number_of_symbols: DWORD,

    /// Размер Optional Header (224 для PE32, 240 для PE32+).
    pub size_of_optional_header: WORD,

    /// Характеристики файла (битовые флаги).
    ///   0x0002 = EXECUTABLE_IMAGE (это .exe, а не .obj)
    ///   0x0020 = LARGE_ADDRESS_AWARE (может использовать >2 GB)
    ///   0x2000 = DLL (это DLL, а не EXE)
    pub characteristics: WORD,
}

// Machine types
pub const IMAGE_FILE_MACHINE_I386: WORD  = 0x014C;
pub const IMAGE_FILE_MACHINE_AMD64: WORD = 0x8664;
pub const IMAGE_FILE_MACHINE_ARM64: WORD = 0xAA64;

// Characteristics
pub const IMAGE_FILE_EXECUTABLE_IMAGE: WORD    = 0x0002;
pub const IMAGE_FILE_LARGE_ADDRESS_AWARE: WORD = 0x0020;
pub const IMAGE_FILE_DLL: WORD                 = 0x2000;

// ---- Optional Header (PE32+ / 64-bit) ----
//
// "Optional" — историческое название, на практике обязателен для .exe/.dll.
// PE32+ — 64-битная версия (для x86_64).

/// Optional Header для PE32+ (64-bit).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageOptionalHeader64 {
    /// Магическое число: 0x10B = PE32, 0x20B = PE32+ (64-bit).
    pub magic: WORD,

    /// Версия линкера.
    pub major_linker_version: BYTE,
    pub minor_linker_version: BYTE,

    /// Размер секции .text (всего кода).
    pub size_of_code: DWORD,

    /// Размер инициализированных данных (.data).
    pub size_of_initialized_data: DWORD,

    /// Размер неинициализированных данных (.bss).
    pub size_of_uninitialized_data: DWORD,

    /// RVA точки входа (main/WinMain/DllMain).
    /// RVA = Relative Virtual Address = смещение от ImageBase.
    pub address_of_entry_point: DWORD,

    /// RVA начала кода.
    pub base_of_code: DWORD,

    // ---- PE32+ specific (в PE32 тут ещё BaseOfData + 32-bit поля) ----

    /// Предпочтительный адрес загрузки.
    /// EXE обычно 0x00400000, DLL — 0x10000000.
    /// Если адрес занят — применяются relocations.
    pub image_base: QWORD,

    /// Выравнивание секций в памяти (обычно 0x1000 = 4 KiB).
    pub section_alignment: DWORD,

    /// Выравнивание секций в файле (обычно 0x200 = 512 байт).
    pub file_alignment: DWORD,

    /// Версия ОС.
    pub major_os_version: WORD,
    pub minor_os_version: WORD,
    pub major_image_version: WORD,
    pub minor_image_version: WORD,
    pub major_subsystem_version: WORD,
    pub minor_subsystem_version: WORD,
    pub win32_version_value: DWORD,

    /// Размер образа в памяти (включая заголовки и все секции, выровнен).
    pub size_of_image: DWORD,

    /// Размер всех заголовков (до начала первой секции).
    pub size_of_headers: DWORD,

    /// Контрольная сумма (для драйверов; обычные .exe = 0).
    pub checksum: DWORD,

    /// Подсистема:
    ///   1 = NATIVE (драйвер)
    ///   2 = WINDOWS_GUI (GUI приложение)
    ///   3 = WINDOWS_CUI (консольное приложение)
    pub subsystem: WORD,

    /// DLL характеристики (ASLR, DEP, ...).
    pub dll_characteristics: WORD,

    /// Размеры стека и кучи (по умолчанию + зарезервировано).
    pub size_of_stack_reserve: QWORD,
    pub size_of_stack_commit: QWORD,
    pub size_of_heap_reserve: QWORD,
    pub size_of_heap_commit: QWORD,

    /// Устарело (всегда 0).
    pub loader_flags: DWORD,

    /// Количество записей в Data Directory.
    pub number_of_rva_and_sizes: DWORD,

    // За этим полем идёт массив DataDirectory[number_of_rva_and_sizes]
    // Мы парсим его отдельно.
}

/// Магические числа Optional Header.
pub const IMAGE_NT_OPTIONAL_HDR32_MAGIC: WORD = 0x10B; // PE32
pub const IMAGE_NT_OPTIONAL_HDR64_MAGIC: WORD = 0x20B; // PE32+

// Subsystem values
pub const IMAGE_SUBSYSTEM_NATIVE: WORD      = 1;
pub const IMAGE_SUBSYSTEM_WINDOWS_GUI: WORD = 2;
pub const IMAGE_SUBSYSTEM_WINDOWS_CUI: WORD = 3;

// ---- Data Directory ----
//
// Массив указателей на важные таблицы внутри PE файла.
// Каждая запись = (RVA, Size).
// Индекс определяет, что это за таблица.

/// Запись Data Directory.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageDataDirectory {
    pub virtual_address: DWORD,  // RVA таблицы
    pub size: DWORD,             // Размер в байтах
}

// Индексы в Data Directory
pub const IMAGE_DIRECTORY_ENTRY_EXPORT: usize         = 0;  // Export Table
pub const IMAGE_DIRECTORY_ENTRY_IMPORT: usize         = 1;  // Import Table
pub const IMAGE_DIRECTORY_ENTRY_RESOURCE: usize       = 2;  // Resource Table
pub const IMAGE_DIRECTORY_ENTRY_EXCEPTION: usize      = 3;  // Exception Table
pub const IMAGE_DIRECTORY_ENTRY_SECURITY: usize       = 4;  // Certificate Table
pub const IMAGE_DIRECTORY_ENTRY_BASERELOC: usize      = 5;  // Base Relocation Table
pub const IMAGE_DIRECTORY_ENTRY_DEBUG: usize           = 6;  // Debug Data
pub const IMAGE_DIRECTORY_ENTRY_TLS: usize            = 9;  // Thread Local Storage
pub const IMAGE_DIRECTORY_ENTRY_IAT: usize            = 12; // Import Address Table
pub const IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT: usize   = 13; // Delay Import

pub const IMAGE_NUMBEROF_DIRECTORY_ENTRIES: usize = 16;

// ---- Section Header ----
//
// Описывает одну секцию PE файла.
// Секция — непрерывный блок кода или данных в файле и в памяти.

/// Заголовок секции (40 байт).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageSectionHeader {
    /// Имя секции (до 8 символов, null-padded).
    /// Примеры: ".text\0\0\0", ".data\0\0\0", ".rdata\0\0"
    pub name: [BYTE; 8],

    /// Размер секции в памяти (до выравнивания).
    pub virtual_size: DWORD,

    /// RVA секции в памяти.
    pub virtual_address: DWORD,

    /// Размер секции в файле (выровнен по FileAlignment).
    pub size_of_raw_data: DWORD,

    /// Смещение секции в файле.
    pub pointer_to_raw_data: DWORD,

    /// Указатели на relocations и line numbers (обычно 0).
    pub pointer_to_relocations: DWORD,
    pub pointer_to_linenumbers: DWORD,
    pub number_of_relocations: WORD,
    pub number_of_linenumbers: WORD,

    /// Характеристики секции (битовые флаги).
    ///   0x00000020 = CODE (содержит код)
    ///   0x00000040 = INITIALIZED_DATA
    ///   0x00000080 = UNINITIALIZED_DATA
    ///   0x20000000 = EXECUTE (можно исполнять)
    ///   0x40000000 = READ (можно читать)
    ///   0x80000000 = WRITE (можно писать)
    pub characteristics: DWORD,
}

// Section characteristics
pub const IMAGE_SCN_CNT_CODE: DWORD               = 0x00000020;
pub const IMAGE_SCN_CNT_INITIALIZED_DATA: DWORD   = 0x00000040;
pub const IMAGE_SCN_CNT_UNINITIALIZED_DATA: DWORD = 0x00000080;
pub const IMAGE_SCN_MEM_EXECUTE: DWORD             = 0x20000000;
pub const IMAGE_SCN_MEM_READ: DWORD                = 0x40000000;
pub const IMAGE_SCN_MEM_WRITE: DWORD               = 0x80000000;

// ---- Import Directory ----
//
// Каждая запись описывает одну DLL и её импорты.

/// Import Directory Entry — описание одной импортируемой DLL.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageImportDescriptor {
    /// RVA Import Lookup Table (или Import Name Table).
    /// Массив указателей на имена/ординалы функций.
    pub original_first_thunk: DWORD,

    /// Timestamp (0 если не bound, -1 если bound).
    pub time_date_stamp: DWORD,

    /// Forwarder chain (-1 если нет форвардинга).
    pub forwarder_chain: DWORD,

    /// RVA строки с именем DLL (null-terminated ASCII).
    /// Пример: "kernel32.dll\0"
    pub name: DWORD,

    /// RVA Import Address Table (IAT).
    /// Загрузчик заменяет записи реальными адресами функций.
    pub first_thunk: DWORD,
}

// ---- Import By Name ----

/// Импорт функции по имени.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageImportByName {
    /// Hint — индекс в Export Table DLL (ускоряет поиск).
    pub hint: WORD,
    // За hint идёт null-terminated ASCII имя функции.
    // name: [u8; ?]  — переменная длина
}

// ---- Export Directory ----
//
// В DLL — описывает все экспортируемые функции.

/// Export Directory Table.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageExportDirectory {
    pub characteristics: DWORD,
    pub time_date_stamp: DWORD,
    pub major_version: WORD,
    pub minor_version: WORD,

    /// RVA имени DLL.
    pub name: DWORD,

    /// Начальный ординал (обычно 1).
    pub base: DWORD,

    /// Количество экспортируемых функций.
    pub number_of_functions: DWORD,

    /// Количество функций с именами.
    pub number_of_names: DWORD,

    /// RVA массива адресов функций (DWORD[NumberOfFunctions]).
    pub address_of_functions: DWORD,

    /// RVA массива RVA имён (DWORD[NumberOfNames]).
    pub address_of_names: DWORD,

    /// RVA массива ординалов (WORD[NumberOfNames]).
    pub address_of_name_ordinals: DWORD,
}

// ---- Base Relocation ----
//
// Если PE загружен не по предпочитаемому ImageBase,
// нужно пропатчить все абсолютные адреса в коде.
// Relocation Table говорит ГДЕ эти адреса.

/// Блок relocations для одной страницы.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ImageBaseRelocation {
    /// RVA страницы (выровнен на 4 KiB).
    pub virtual_address: DWORD,

    /// Размер блока (включая этот заголовок).
    pub size_of_block: DWORD,

    // За заголовком идёт массив WORD entries:
    //   верхние 4 бита = тип relocation
    //   нижние 12 бит  = смещение внутри страницы
}

// Типы relocations
pub const IMAGE_REL_BASED_ABSOLUTE: WORD = 0;  // Пропустить (padding)
pub const IMAGE_REL_BASED_HIGH: WORD     = 1;  // Патч верхних 16 бит
pub const IMAGE_REL_BASED_LOW: WORD      = 2;  // Патч нижних 16 бит
pub const IMAGE_REL_BASED_HIGHLOW: WORD  = 3;  // Патч 32 бита (PE32)
pub const IMAGE_REL_BASED_DIR64: WORD    = 10; // Патч 64 бита (PE32+)

// =============================================================================
// Валидация PE — быстрая проверка, что файл является корректным PE
// =============================================================================

use super::error::*;
use super::types::NTSTATUS;

/// Проверить DOS Header.
pub fn validate_dos_header(data: &[u8]) -> Result<&ImageDosHeader, NTSTATUS> {
    if data.len() < core::mem::size_of::<ImageDosHeader>() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    let dos = unsafe { &*(data.as_ptr() as *const ImageDosHeader) };

    if dos.e_magic != IMAGE_DOS_SIGNATURE {
        return Err(STATUS_INVALID_IMAGE_NOT_MZ);
    }

    if dos.e_lfanew < 0 || dos.e_lfanew as usize >= data.len() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    Ok(dos)
}

/// Проверить PE Signature + COFF Header.
pub fn validate_pe_header(data: &[u8], dos: &ImageDosHeader) -> Result<&ImageFileHeader, NTSTATUS> {
    let pe_offset = dos.e_lfanew as usize;

    // Проверяем PE signature ("PE\0\0")
    if pe_offset + 4 > data.len() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }
    let pe_sig = u32::from_le_bytes([
        data[pe_offset], data[pe_offset + 1],
        data[pe_offset + 2], data[pe_offset + 3],
    ]);
    if pe_sig != IMAGE_NT_SIGNATURE {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    // COFF Header начинается сразу после signature
    let coff_offset = pe_offset + 4;
    if coff_offset + core::mem::size_of::<ImageFileHeader>() > data.len() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    let coff = unsafe { &*(data.as_ptr().add(coff_offset) as *const ImageFileHeader) };

    Ok(coff)
}

/// Проверить Optional Header (PE32+).
pub fn validate_optional_header(
    data: &[u8],
    dos: &ImageDosHeader,
) -> Result<&ImageOptionalHeader64, NTSTATUS> {
    let opt_offset = dos.e_lfanew as usize + 4 + core::mem::size_of::<ImageFileHeader>();

    if opt_offset + core::mem::size_of::<ImageOptionalHeader64>() > data.len() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    let opt = unsafe { &*(data.as_ptr().add(opt_offset) as *const ImageOptionalHeader64) };

    if opt.magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC {
        return Err(STATUS_IMAGE_MACHINE_TYPE_MISMATCH);
    }

    Ok(opt)
}

/// Получить массив Section Headers.
pub fn get_section_headers(
    data: &[u8],
    dos: &ImageDosHeader,
    coff: &ImageFileHeader,
) -> Result<&[ImageSectionHeader], NTSTATUS> {
    let sections_offset = dos.e_lfanew as usize
        + 4
        + core::mem::size_of::<ImageFileHeader>()
        + coff.size_of_optional_header as usize;

    let num = coff.number_of_sections as usize;
    let total_size = num * core::mem::size_of::<ImageSectionHeader>();

    if sections_offset + total_size > data.len() {
        return Err(STATUS_INVALID_IMAGE_FORMAT);
    }

    let sections = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(sections_offset) as *const ImageSectionHeader,
            num,
        )
    };

    Ok(sections)
}

/// Полная валидация PE файла.
/// Возвращает информацию для загрузчика.
pub struct PeInfo<'a> {
    pub dos: &'a ImageDosHeader,
    pub coff: &'a ImageFileHeader,
    pub optional: &'a ImageOptionalHeader64,
    pub sections: &'a [ImageSectionHeader],
}

pub fn parse_pe(data: &[u8]) -> Result<PeInfo<'_>, NTSTATUS> {
    let dos = validate_dos_header(data)?;
    let coff = validate_pe_header(data, dos)?;
    let optional = validate_optional_header(data, dos)?;
    let sections = get_section_headers(data, dos, coff)?;

    Ok(PeInfo { dos, coff, optional, sections })
}
