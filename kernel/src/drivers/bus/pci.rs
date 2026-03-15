// =============================================================================
// NoNameOS — PCI Bus Driver
// =============================================================================
//
// PCI (Peripheral Component Interconnect) — основная шина в x86 системах.
// Через неё подключены: GPU, NVMe, сетевые карты, USB контроллеры и т.д.
//
// Как CPU общается с PCI устройствами:
//
//   Метод 1: I/O порты (Legacy, но работает везде)
//     Порт 0xCF8 — CONFIG_ADDRESS (адрес регистра)
//     Порт 0xCFC — CONFIG_DATA    (данные)
//
//   Метод 2: MMIO (PCIe ECAM) — маппинг конфигурации в память
//     Быстрее, поддерживает extended config space (4096 байт вместо 256).
//     Адрес берётся из ACPI таблицы MCFG.
//
//   Мы начинаем с метода 1 (I/O порты) — он проще и работает в QEMU.
//
// Адресация PCI устройства:
//   Bus    (0-255)  — номер шины
//   Device (0-31)   — номер слота на шине
//   Function (0-7)  — функция внутри устройства (multi-function devices)
//
// CONFIG_ADDRESS формат (32 бита):
//   ┌──────┬──────────┬────────┬──────────┬──────────┬───┐
//   │ Bit  │  31      │ 23..16 │ 15..11   │ 10..8    │7.2│
//   │ Name │ Enable   │  Bus   │ Device   │ Function │Reg│
//   └──────┴──────────┴────────┴──────────┴──────────┴───┘
//
// PCI Configuration Space (256 байт на устройство):
//   Offset 0x00: Vendor ID (16 бит)  — производитель (0x8086 = Intel, 0x10DE = NVIDIA)
//   Offset 0x02: Device ID (16 бит)  — конкретное устройство
//   Offset 0x04: Command   (16 бит)  — управление устройством
//   Offset 0x06: Status    (16 бит)  — статус
//   Offset 0x08: Revision  (8 бит)   — ревизия
//   Offset 0x09: Prog IF   (8 бит)   — programming interface
//   Offset 0x0A: Subclass  (8 бит)   — подкласс
//   Offset 0x0B: Class     (8 бит)   — класс устройства
//   Offset 0x0E: Header Type (8 бит) — тип заголовка (0 = обычный, 1 = мост)
//   Offset 0x10-0x27: BAR0-BAR5      — Base Address Registers (MMIO/IO адреса)
//   Offset 0x2C: Subsystem Vendor ID
//   Offset 0x2E: Subsystem ID
//   Offset 0x3C: Interrupt Line (IRQ)
//   Offset 0x3D: Interrupt Pin
//
// PCI Class Codes (основные):
//   0x01 = Mass Storage (SATA, NVMe, IDE)
//   0x02 = Network Controller
//   0x03 = Display Controller (GPU!) ← нам нужен этот
//   0x04 = Multimedia (Audio)
//   0x06 = Bridge (PCI-to-PCI, Host)
//   0x0C = Serial Bus (USB, FireWire)
//
// BAR (Base Address Register):
//   Указывает, где в адресном пространстве находятся регистры устройства.
//   Бит 0: 0 = Memory-mapped (MMIO), 1 = I/O port
//   Биты 2..1 (для MMIO): 00 = 32-bit, 10 = 64-bit
//   Для определения размера: записываем 0xFFFFFFFF, читаем обратно, инвертируем.
//
// Источники:
//   - PCI Local Bus Specification 3.0
//   - Linux: drivers/pci/access.c, drivers/pci/probe.c
//   - OSDev wiki: PCI
// =============================================================================

use super::super::{Device, DeviceClass, BusType, register_device};

// ---- I/O порты для PCI Configuration ----

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16    = 0xCFC;

// ---- PCI Class Codes ----

pub const PCI_CLASS_STORAGE: u8    = 0x01;
pub const PCI_CLASS_NETWORK: u8    = 0x02;
pub const PCI_CLASS_DISPLAY: u8    = 0x03;
pub const PCI_CLASS_MULTIMEDIA: u8 = 0x04;
pub const PCI_CLASS_BRIDGE: u8     = 0x06;
pub const PCI_CLASS_SERIAL: u8     = 0x0C;

// ---- Помощники для I/O портов ----

/// Записать 32-битное значение в I/O порт.
#[inline(always)]
unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Прочитать 32-битное значение из I/O порта.
#[inline(always)]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") value,
        options(nomem, nostack, preserves_flags)
    );
    value
}

// ---- PCI Configuration Space Access ----

/// Сформировать CONFIG_ADDRESS для обращения к PCI конфигурации.
fn pci_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1 << 31)                          // Enable bit
        | ((bus as u32) << 16)
        | ((device as u32 & 0x1F) << 11)
        | ((function as u32 & 0x07) << 8)
        | ((offset as u32) & 0xFC)     // 4-byte aligned
}

/// Прочитать 32 бита из PCI configuration space.
pub fn pci_config_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        outl(PCI_CONFIG_ADDRESS, pci_address(bus, dev, func, offset));
        inl(PCI_CONFIG_DATA)
    }
}

/// Записать 32 бита в PCI configuration space.
pub fn pci_config_write32(bus: u8, dev: u8, func: u8, offset: u8, value: u32) {
    unsafe {
        outl(PCI_CONFIG_ADDRESS, pci_address(bus, dev, func, offset));
        outl(PCI_CONFIG_DATA, value);
    }
}

/// Прочитать 16 бит из PCI configuration space.
pub fn pci_config_read16(bus: u8, dev: u8, func: u8, offset: u8) -> u16 {
    let val32 = pci_config_read32(bus, dev, func, offset & 0xFC);
    ((val32 >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Прочитать 8 бит из PCI configuration space.
pub fn pci_config_read8(bus: u8, dev: u8, func: u8, offset: u8) -> u8 {
    let val32 = pci_config_read32(bus, dev, func, offset & 0xFC);
    ((val32 >> ((offset & 3) * 8)) & 0xFF) as u8
}

// ---- PCI Device Info ----

/// Информация об обнаруженном PCI устройстве.
#[derive(Debug, Clone, Copy)]
pub struct PciDeviceInfo {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub irq_line: u8,
    pub bar: [u32; 6],
}

impl PciDeviceInfo {
    /// Прочитать информацию о PCI устройстве из config space.
    pub fn read(bus: u8, dev: u8, func: u8) -> Option<Self> {
        let vendor_id = pci_config_read16(bus, dev, func, 0x00);
        if vendor_id == 0xFFFF {
            return None; // Устройства нет
        }

        let device_id = pci_config_read16(bus, dev, func, 0x02);
        let revision  = pci_config_read8(bus, dev, func, 0x08);
        let prog_if   = pci_config_read8(bus, dev, func, 0x09);
        let subclass  = pci_config_read8(bus, dev, func, 0x0A);
        let class_code = pci_config_read8(bus, dev, func, 0x0B);
        let header_type = pci_config_read8(bus, dev, func, 0x0E);
        let irq_line  = pci_config_read8(bus, dev, func, 0x3C);

        let mut bar = [0u32; 6];
        // BAR только для обычных устройств (header type 0)
        if header_type & 0x7F == 0 {
            for i in 0..6 {
                bar[i] = pci_config_read32(bus, dev, func, 0x10 + (i as u8) * 4);
            }
        }

        Some(PciDeviceInfo {
            bus, device: dev, function: func,
            vendor_id, device_id,
            class_code, subclass, prog_if, revision,
            header_type, irq_line, bar,
        })
    }

    /// Получить базовый MMIO адрес из BAR (32-bit или 64-bit).
    pub fn bar_address(&self, bar_idx: usize) -> u64 {
        if bar_idx >= 6 { return 0; }
        let raw = self.bar[bar_idx];
        if raw & 1 != 0 {
            // I/O space BAR
            return (raw & 0xFFFFFFFC) as u64;
        }
        // Memory space BAR
        let bar_type = (raw >> 1) & 0x03;
        let base = (raw & 0xFFFFFFF0) as u64;
        if bar_type == 0x02 && bar_idx + 1 < 6 {
            // 64-bit BAR: объединяем с следующим BAR
            let high = self.bar[bar_idx + 1] as u64;
            base | (high << 32)
        } else {
            base
        }
    }

    /// Получить размер BAR региона.
    pub fn bar_size(&self, bar_idx: usize) -> u64 {
        if bar_idx >= 6 { return 0; }
        let bus = self.bus;
        let dev = self.device;
        let func = self.function;
        let offset = 0x10 + (bar_idx as u8) * 4;

        let original = pci_config_read32(bus, dev, func, offset);
        pci_config_write32(bus, dev, func, offset, 0xFFFFFFFF);
        let size_mask = pci_config_read32(bus, dev, func, offset);
        pci_config_write32(bus, dev, func, offset, original); // восстанавливаем

        if size_mask == 0 || size_mask == 0xFFFFFFFF {
            return 0;
        }

        if original & 1 != 0 {
            // I/O BAR
            let mask = size_mask & 0xFFFFFFFC;
            (!mask).wrapping_add(1) as u64
        } else {
            // Memory BAR
            let mask = size_mask & 0xFFFFFFF0;
            (!mask).wrapping_add(1) as u64
        }
    }

    /// Перевести PCI class code в наш DeviceClass.
    pub fn to_device_class(&self) -> DeviceClass {
        match self.class_code {
            PCI_CLASS_DISPLAY    => DeviceClass::Display,
            PCI_CLASS_NETWORK    => DeviceClass::Network,
            PCI_CLASS_STORAGE    => DeviceClass::Storage,
            PCI_CLASS_MULTIMEDIA => DeviceClass::Audio,
            PCI_CLASS_SERIAL     => DeviceClass::Usb,
            _ => DeviceClass::Unknown,
        }
    }
}

// ---- PCI Bus Scan ----

/// Результат сканирования PCI шины.
const MAX_PCI_DEVICES: usize = 32;

static mut PCI_DEVICES: [PciDeviceInfo; MAX_PCI_DEVICES] = [PciDeviceInfo {
    bus: 0, device: 0, function: 0,
    vendor_id: 0, device_id: 0,
    class_code: 0, subclass: 0, prog_if: 0, revision: 0,
    header_type: 0, irq_line: 0,
    bar: [0; 6],
}; MAX_PCI_DEVICES];

static mut PCI_DEVICE_COUNT: usize = 0;

/// Сканировать все PCI шины и обнаружить устройства.
///
/// Это брутфорс-сканирование: перебираем все возможные bus/device/function.
/// В продакшене используется рекурсивное сканирование через PCI мосты.
/// Для QEMU и начального этапа — достаточно bus 0.
pub fn scan() -> usize {
    let mut count = 0;

    for bus in 0..=255u16 {
        for dev in 0..32u8 {
            // Проверяем function 0
            if let Some(info) = PciDeviceInfo::read(bus as u8, dev, 0) {
                if count < MAX_PCI_DEVICES {
                    unsafe {
                        PCI_DEVICES[count] = info;
                        PCI_DEVICE_COUNT = count + 1;
                    }
                    count += 1;

                    // Регистрируем в общем device manager
                    register_pci_device(&info);

                    // Если multi-function — проверяем остальные функции
                    if info.header_type & 0x80 != 0 {
                        for func in 1..8u8 {
                            if let Some(finfo) = PciDeviceInfo::read(bus as u8, dev, func) {
                                if count < MAX_PCI_DEVICES {
                                    unsafe {
                                        PCI_DEVICES[count] = finfo;
                                        PCI_DEVICE_COUNT = count + 1;
                                    }
                                    count += 1;
                                    register_pci_device(&finfo);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    count
}

/// Зарегистрировать PCI устройство в глобальном DeviceManager.
fn register_pci_device(info: &PciDeviceInfo) {
    let mut dev = Device::empty();
    dev.class = info.to_device_class();
    dev.bus = BusType::Pci;
    dev.vendor_id = info.vendor_id;
    dev.device_id = info.device_id;
    dev.irq = info.irq_line as u32;

    // Берём MMIO из BAR0 (основной ресурс для большинства устройств)
    let bar0 = info.bar[0];
    if bar0 != 0 && (bar0 & 1 == 0) {
        dev.mmio_base = info.bar_address(0);
        dev.mmio_size = info.bar_size(0);
    }

    // Формируем имя: "pci:VVVV:DDDD"
    let mut name_buf = [0u8; 32];
    let prefix = b"pci:";
    name_buf[..4].copy_from_slice(prefix);
    hex16(info.vendor_id, &mut name_buf[4..8]);
    name_buf[8] = b':';
    hex16(info.device_id, &mut name_buf[9..13]);
    dev.name[..13].copy_from_slice(&name_buf[..13]);
    dev.name_len = 13;

    register_device(dev);
}

/// Получить информацию о PCI устройстве по индексу.
pub fn get_pci_device(idx: usize) -> Option<&'static PciDeviceInfo> {
    unsafe {
        if idx < PCI_DEVICE_COUNT {
            Some(&PCI_DEVICES[idx])
        } else {
            None
        }
    }
}

/// Количество обнаруженных PCI устройств.
pub fn pci_device_count() -> usize {
    unsafe { PCI_DEVICE_COUNT }
}

// ---- Enable Bus Mastering ----

/// Включить Bus Mastering для PCI устройства.
/// Необходимо для DMA — устройство сможет читать/писать RAM напрямую.
/// Без этого GPU не сможет работать.
pub fn enable_bus_master(info: &PciDeviceInfo) {
    let cmd = pci_config_read16(info.bus, info.device, info.function, 0x04);
    // Бит 2 = Bus Master Enable, Бит 1 = Memory Space Enable, Бит 0 = I/O Space Enable
    let new_cmd = cmd | 0x07;
    let full = pci_config_read32(info.bus, info.device, info.function, 0x04);
    let updated = (full & 0xFFFF0000) | (new_cmd as u32);
    pci_config_write32(info.bus, info.device, info.function, 0x04, updated);
}

// ---- Утилита ----

/// Записать 16-битное число как 4 hex символа в буфер.
fn hex16(val: u16, buf: &mut [u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if buf.len() >= 4 {
        buf[0] = HEX[((val >> 12) & 0xF) as usize];
        buf[1] = HEX[((val >> 8) & 0xF) as usize];
        buf[2] = HEX[((val >> 4) & 0xF) as usize];
        buf[3] = HEX[(val & 0xF) as usize];
    }
}
