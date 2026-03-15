// =============================================================================
// NoNameOS — Multiboot2 Info Structure Parser
// =============================================================================
//
// Парсит структуру, переданную GRUB2 при загрузке.
// Ядро получает указатель на неё через RSI в kernel_main().
//
// Формат:
//   [total_size: u32] [reserved: u32]
//   [tag0] [tag1] ... [end_tag]
//
// Каждый тег:
//   [type: u32] [size: u32] [payload...] [padding до align 8]
//
// Нас интересует:
//   - Tag type 8: Framebuffer info
//   - Tag type 6: Memory map
//   - Tag type 1: Boot command line
//
// =============================================================================

// ---- Tag types ----

pub const TAG_END: u32            = 0;
pub const TAG_CMDLINE: u32        = 1;
pub const TAG_BOOT_LOADER: u32    = 2;
pub const TAG_MODULE: u32         = 3;
pub const TAG_BASIC_MEMINFO: u32  = 4;
pub const TAG_BOOTDEV: u32        = 5;
pub const TAG_MMAP: u32           = 6;
pub const TAG_FRAMEBUFFER: u32    = 8;
pub const TAG_ELF_SECTIONS: u32   = 9;
pub const TAG_APM: u32            = 10;
pub const TAG_EFI32: u32          = 11;
pub const TAG_EFI64: u32          = 12;
pub const TAG_ACPI_OLD: u32       = 14;
pub const TAG_ACPI_NEW: u32       = 15;

// ---- Tag header ----

#[repr(C, packed)]
pub struct TagHeader {
    pub tag_type: u32,
    pub size: u32,
}

// ---- Framebuffer tag (type 8) ----

#[repr(C, packed)]
pub struct FramebufferTag {
    pub tag_type: u32,
    pub size: u32,
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub fb_type: u8,    // 0=indexed, 1=RGB, 2=EGA text
    pub reserved: u8,
}

/// Тип framebuffer
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FramebufferType {
    Indexed,
    Rgb,
    EgaText,
    Unknown(u8),
}

/// Информация о framebuffer, извлечённая из Multiboot2
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u8,
    pub fb_type: FramebufferType,
}

// ---- Iterator по тегам ----

/// Итератор по Multiboot2 тегам.
pub struct TagIterator {
    current: *const u8,
    end: *const u8,
}

impl TagIterator {
    /// Создать итератор из указателя на Multiboot2 info struct.
    ///
    /// # Safety
    /// `info_addr` должен быть валидным указателем на Multiboot2 info.
    pub unsafe fn new(info_addr: u64) -> Self {
        let ptr = info_addr as *const u8;
        let total_size = *(ptr as *const u32);
        TagIterator {
            current: ptr.add(8), // пропускаем total_size + reserved
            end: ptr.add(total_size as usize),
        }
    }
}

impl Iterator for TagIterator {
    type Item = *const TagHeader;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.end {
            return None;
        }

        let tag = self.current as *const TagHeader;
        let tag_type = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tag).tag_type)) };
        let tag_size = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tag).size)) };

        if tag_type == TAG_END {
            return None;
        }

        if tag_size < 8 {
            return None; // защита от бесконечного цикла
        }

        let result = tag;

        // Следующий тег — выровнен на 8 байт
        let next_offset = ((tag_size as usize) + 7) & !7;
        self.current = unsafe { self.current.add(next_offset) };

        Some(result)
    }
}

// ---- Высокоуровневый API ----

/// Найти framebuffer info в Multiboot2 tags.
///
/// # Safety
/// `info_addr` должен быть валидным Multiboot2 info pointer.
pub unsafe fn find_framebuffer(info_addr: u64) -> Option<FramebufferInfo> {
    let iter = TagIterator::new(info_addr);

    for tag_ptr in iter {
        let tag_type = core::ptr::read_unaligned(core::ptr::addr_of!((*tag_ptr).tag_type));

        if tag_type == TAG_FRAMEBUFFER {
            let fb = tag_ptr as *const FramebufferTag;

            let addr = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).addr));
            let pitch = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).pitch));
            let width = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).width));
            let height = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).height));
            let bpp = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).bpp));
            let fb_type_raw = core::ptr::read_unaligned(core::ptr::addr_of!((*fb).fb_type));

            let fb_type = match fb_type_raw {
                0 => FramebufferType::Indexed,
                1 => FramebufferType::Rgb,
                2 => FramebufferType::EgaText,
                x => FramebufferType::Unknown(x),
            };

            return Some(FramebufferInfo {
                addr,
                pitch,
                width,
                height,
                bpp,
                fb_type,
            });
        }
    }

    None
}

/// Получить boot command line из Multiboot2 tags.
///
/// # Safety
/// `info_addr` должен быть валидным Multiboot2 info pointer.
pub unsafe fn find_cmdline(info_addr: u64) -> Option<&'static str> {
    let iter = TagIterator::new(info_addr);

    for tag_ptr in iter {
        let tag_type = core::ptr::read_unaligned(core::ptr::addr_of!((*tag_ptr).tag_type));

        if tag_type == TAG_CMDLINE {
            let tag_size = core::ptr::read_unaligned(core::ptr::addr_of!((*tag_ptr).size)) as usize;
            if tag_size <= 8 { return None; }

            let str_ptr = (tag_ptr as *const u8).add(8);
            let str_len = tag_size - 8;

            // Ищем null-terminator
            let mut len = 0;
            while len < str_len && *str_ptr.add(len) != 0 {
                len += 1;
            }

            let bytes = core::slice::from_raw_parts(str_ptr, len);
            return core::str::from_utf8(bytes).ok();
        }
    }

    None
}
