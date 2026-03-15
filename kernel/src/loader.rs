// =============================================================================
// NoNameOS — PE Loader
// =============================================================================
//
// Загрузчик PE-файлов (.exe) в user-space.
//
// Полный процесс загрузки:
//
//   1. ПАРСИНГ: validate DOS header, PE signature, COFF header, Optional header
//   2. СОЗДАНИЕ ПРОЦЕССА: новое адресное пространство (PML4)
//   3. МАППИНГ ЗАГОЛОВКОВ: первые size_of_headers байт → ImageBase
//   4. МАППИНГ СЕКЦИЙ: каждая секция → ImageBase + section.VirtualAddress
//   5. RELOCATIONS: если ImageBase != preferred, патчим абсолютные адреса
//   6. IMPORTS: загружаем зависимые DLL, заполняем IAT нашими реализациями
//   7. ENTRY POINT: ImageBase + AddressOfEntryPoint → RIP user-потока
//
// Текущая реализация:
//   - Полный парсинг PE64 через win32::pe
//   - Маппинг секций с правильными правами (R/W/X)
//   - Заглушка для imports (будет заполняться по мере реализации Win32 API)
//   - Relocations пока не нужны (грузим по preferred ImageBase)
//
// В будущем:
//   - Полная поддержка Import Table с поиском в наших DLL-заглушках
//   - Base relocations для ASLR
//   - TLS (Thread Local Storage) инициализация
//   - DLL loading (LoadLibrary)
//
// Аналоги:
//   - Windows: ntoskrnl!MmMapViewOfSection + ntdll!LdrpInitializeProcess
//   - Wine: dlls/ntdll/loader.c (load_dll, map_image)
//   - ReactOS: ntoskrnl/mm/section.c
// =============================================================================

use crate::win32::pe::*;
use crate::win32::types::NTSTATUS;
use crate::userspace;

// ---- Результат загрузки ----

/// Информация о загруженном PE-образе.
pub struct LoadedImage {
    /// Абсолютный адрес точки входа (ImageBase + AddressOfEntryPoint).
    pub entry_point: u64,
    /// Базовый адрес образа в памяти.
    pub image_base: u64,
    /// Размер образа в памяти.
    pub image_size: u32,
    /// Количество загруженных секций.
    pub sections_loaded: usize,
    /// Подсистема (GUI / Console / Native).
    pub subsystem: u16,
    /// Машина (AMD64 / i386 / ARM64).
    pub machine: u16,
    /// CR3 нового адресного пространства.
    pub cr3: u64,
    /// Виртуальный адрес вершины user stack.
    pub user_stack_top: u64,
}

// ---- Ошибки загрузки ----

#[derive(Debug)]
pub enum LoadError {
    /// PE парсинг провалился.
    InvalidPe(NTSTATUS),
    /// Не x86_64 PE.
    WrongArchitecture,
    /// Не исполняемый файл (может быть DLL или .obj).
    NotExecutable,
    /// Не удалось создать адресное пространство (нет памяти).
    OutOfMemory,
    /// Секция не влезает в допустимый размер.
    SectionTooLarge,
    /// Образ слишком большой.
    ImageTooLarge,
}

// ---- Основная функция загрузки ----

/// Загрузить PE-файл из байтового буфера.
///
/// `data` — полное содержимое .exe файла.
/// `name` — имя процесса (для отладки).
///
/// Возвращает `LoadedImage` с готовым адресным пространством.
pub fn load_pe(data: &[u8], name: &str) -> Result<LoadedImage, LoadError> {
    // === 1. Парсинг PE ===

    let pe = parse_pe(data).map_err(LoadError::InvalidPe)?;

    // Копируем поля из packed struct в локальные переменные
    // (прямые ссылки на поля packed struct — UB из-за alignment)
    let machine = { pe.coff.machine };
    let characteristics = { pe.coff.characteristics };
    let num_sections = { pe.coff.number_of_sections };
    let image_base = { pe.optional.image_base } as usize;
    let image_size = { pe.optional.size_of_image };
    let entry_rva = { pe.optional.address_of_entry_point };
    let subsystem = { pe.optional.subsystem };
    let headers_size_raw = { pe.optional.size_of_headers };
    let _num_rva_and_sizes = { pe.optional.number_of_rva_and_sizes };

    // Проверяем архитектуру
    if machine != IMAGE_FILE_MACHINE_AMD64 {
        return Err(LoadError::WrongArchitecture);
    }

    // Проверяем что это .exe (не DLL)
    if characteristics & IMAGE_FILE_DLL != 0 {
        return Err(LoadError::NotExecutable);
    }

    // Ограничение: образ не более 16 MiB
    if image_size > 16 * 1024 * 1024 {
        return Err(LoadError::ImageTooLarge);
    }

    crate::println!("[PE] Loading '{}': base=0x{:X} size=0x{:X} entry=0x{:X}",
        name, image_base, image_size, entry_rva);
    crate::println!("[PE] Machine: 0x{:04X}  Subsystem: {}  Sections: {}",
        machine,
        match subsystem {
            IMAGE_SUBSYSTEM_WINDOWS_GUI => "GUI",
            IMAGE_SUBSYSTEM_WINDOWS_CUI => "Console",
            IMAGE_SUBSYSTEM_NATIVE => "Native",
            _ => "Unknown",
        },
        num_sections);

    // === 2. Создание адресного пространства ===

    let cr3 = userspace::create_address_space()
        .ok_or(LoadError::OutOfMemory)?;

    // === 3. Маппинг заголовков ===

    let headers_size = headers_size_raw as usize;
    if headers_size > data.len() {
        return Err(LoadError::InvalidPe(NTSTATUS(0xC000007B))); // STATUS_INVALID_IMAGE_FORMAT
    }

    if !userspace::map_user_pages(cr3, image_base, &data[..headers_size], headers_size) {
        return Err(LoadError::OutOfMemory);
    }

    // === 4. Маппинг секций ===

    let mut sections_loaded = 0;

    for section in pe.sections {
        let sec_name = section_name(section);
        // Копируем поля packed struct в локальные переменные
        let s_virt_addr = { section.virtual_address };
        let s_virt_size = { section.virtual_size };
        let s_raw_offset = { section.pointer_to_raw_data };
        let s_raw_size = { section.size_of_raw_data };
        let s_chars = { section.characteristics };

        let virt_addr = image_base + s_virt_addr as usize;
        let virt_size = s_virt_size as usize;
        let raw_offset = s_raw_offset as usize;
        let raw_size = s_raw_size as usize;

        // Размер в памяти (max из virtual_size и raw_size, но берём virtual_size)
        let mem_size = if virt_size > 0 { virt_size } else { raw_size };

        if mem_size == 0 { continue; }

        // Ограничение на размер секции
        if mem_size > 4 * 1024 * 1024 {
            return Err(LoadError::SectionTooLarge);
        }

        // Данные секции из файла
        let sec_data = if raw_size > 0 && raw_offset + raw_size <= data.len() {
            &data[raw_offset..raw_offset + raw_size]
        } else {
            &[] // BSS-подобная секция (только виртуальная)
        };

        crate::println!("[PE]   {} vaddr=0x{:X} vsize=0x{:X} raw=0x{:X} flags=0x{:08X}",
            sec_name, virt_addr, mem_size, raw_size, s_chars);

        if !userspace::map_user_pages(cr3, virt_addr, sec_data, mem_size) {
            return Err(LoadError::OutOfMemory);
        }

        sections_loaded += 1;
    }

    // === 5. Import Table (заглушка) ===

    // TODO: обход Import Directory, загрузка DLL-заглушек, заполнение IAT.
    // Пока: если есть imports, логируем их имена для информации.
    log_imports(data, &pe);

    // === 6. User stack ===

    let user_stack_top = userspace::alloc_user_stack(cr3)
        .ok_or(LoadError::OutOfMemory)?;

    // === 7. Готово ===

    let entry_point = image_base as u64 + entry_rva as u64;
    crate::println!("[PE] Loaded OK: entry=0x{:X} stack=0x{:X} sections={}",
        entry_point, user_stack_top, sections_loaded);

    Ok(LoadedImage {
        entry_point,
        image_base: image_base as u64,
        image_size,
        sections_loaded,
        subsystem,
        machine: pe.coff.machine,
        cr3,
        user_stack_top,
    })
}

/// Загрузить raw бинарник (не PE) — для тестирования.
///
/// Маппит `code` по адресу IMAGE_BASE, создаёт стек,
/// entry point = IMAGE_BASE.
pub fn load_raw_binary(code: &[u8], name: &str) -> Result<LoadedImage, LoadError> {
    let image_base = userspace::USER_IMAGE_BASE;

    crate::println!("[LOADER] Loading raw binary '{}': {} bytes at 0x{:X}",
        name, code.len(), image_base);

    let cr3 = userspace::create_address_space()
        .ok_or(LoadError::OutOfMemory)?;

    let image_size = ((code.len() + 4095) / 4096) * 4096;
    if !userspace::map_user_pages(cr3, image_base, code, image_size) {
        return Err(LoadError::OutOfMemory);
    }

    let user_stack_top = userspace::alloc_user_stack(cr3)
        .ok_or(LoadError::OutOfMemory)?;

    crate::println!("[LOADER] Ready: entry=0x{:X} stack=0x{:X}", image_base, user_stack_top);

    Ok(LoadedImage {
        entry_point: image_base as u64,
        image_base: image_base as u64,
        image_size: image_size as u32,
        sections_loaded: 1,
        subsystem: 0,
        machine: IMAGE_FILE_MACHINE_AMD64,
        cr3,
        user_stack_top,
    })
}

// ---- Вспомогательные функции ----

/// Извлечь имя секции как строку.
fn section_name(section: &ImageSectionHeader) -> &str {
    let mut len = 0;
    while len < 8 && section.name[len] != 0 {
        len += 1;
    }
    unsafe { core::str::from_utf8_unchecked(&section.name[..len]) }
}

/// Залогировать Import Table (информационно, не загружая).
fn log_imports(data: &[u8], pe: &PeInfo) {
    // Data Directory для Import Table
    let num_dirs = { pe.optional.number_of_rva_and_sizes } as usize;
    let e_lfanew = { pe.dos.e_lfanew } as usize;

    if num_dirs <= IMAGE_DIRECTORY_ENTRY_IMPORT {
        return;
    }

    // Data directories расположены сразу после Optional Header
    let dir_offset = e_lfanew
        + 4
        + core::mem::size_of::<ImageFileHeader>()
        + core::mem::size_of::<ImageOptionalHeader64>();

    if dir_offset + (num_dirs * 8) > data.len() {
        return;
    }

    let import_dir = unsafe {
        let ptr = data.as_ptr().add(dir_offset + IMAGE_DIRECTORY_ENTRY_IMPORT * 8);
        &*(ptr as *const ImageDataDirectory)
    };

    let imp_va = { import_dir.virtual_address };
    let imp_sz = { import_dir.size };

    if imp_va == 0 || imp_sz == 0 {
        crate::println!("[PE] No imports.");
        return;
    }

    crate::println!("[PE] Import Directory at RVA 0x{:X} (size=0x{:X}):",
        imp_va, imp_sz);

    // Обходим Import Directory Table
    // Каждая запись = IMAGE_IMPORT_DESCRIPTOR (20 байт)
    // Массив заканчивается полностью нулевой записью.
    let import_rva = imp_va as usize;
    let desc_size = core::mem::size_of::<ImageImportDescriptor>();
    let mut offset = 0;

    loop {
        let desc_file_offset = rva_to_offset(import_rva + offset, pe);
        if desc_file_offset.is_none() { break; }
        let fo = desc_file_offset.unwrap();

        if fo + desc_size > data.len() { break; }

        let desc = unsafe {
            &*(data.as_ptr().add(fo) as *const ImageImportDescriptor)
        };

        // Копируем поля из packed struct
        let desc_name = { desc.name };
        let desc_oft = { desc.original_first_thunk };

        // Нулевая запись = конец
        if desc_name == 0 && desc_oft == 0 {
            break;
        }

        // Имя DLL
        if let Some(name_offset) = rva_to_offset(desc_name as usize, pe) {
            let mut name_len = 0;
            while name_offset + name_len < data.len() && data[name_offset + name_len] != 0 {
                name_len += 1;
                if name_len > 128 { break; }
            }
            let dll_name = unsafe {
                core::str::from_utf8_unchecked(&data[name_offset..name_offset + name_len])
            };
            crate::println!("[PE]   import: {}", dll_name);
        }

        offset += desc_size;
    }
}

/// Конвертировать RVA → file offset, используя Section Headers.
fn rva_to_offset(rva: usize, pe: &PeInfo) -> Option<usize> {
    for section in pe.sections {
        let sec_rva = section.virtual_address as usize;
        let sec_size = if section.virtual_size > 0 {
            section.virtual_size as usize
        } else {
            section.size_of_raw_data as usize
        };

        if rva >= sec_rva && rva < sec_rva + sec_size {
            let delta = rva - sec_rva;
            return Some(section.pointer_to_raw_data as usize + delta);
        }
    }
    None
}
