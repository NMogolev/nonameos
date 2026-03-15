// =============================================================================
// NoNameOS — Driver Framework
// =============================================================================
//
// Модель устройств, совместимая с концепциями Linux device model.
// Цель: дать возможность портировать Linux-драйверы с минимальным shim.
//
// Архитектура Linux device model:
//
//   ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//   │   Bus Type   │────│   Device     │────│   Driver     │
//   │ (pci, usb..) │    │ (конкретное  │    │ (код для     │
//   │              │    │  устройство) │    │  устройства) │
//   └─────────────┘     └─────────────┘     └─────────────┘
//
//   Bus — тип шины (PCI, USB, Platform...)
//   Device — экземпляр устройства, обнаруженный на шине
//   Driver — код, который умеет работать с устройством
//
//   Когда Bus обнаруживает Device, он ищет подходящий Driver (match).
//   Если нашёл — вызывает driver.probe(device).
//   При отключении — driver.remove(device).
//
// В Linux это struct bus_type, struct device, struct device_driver.
// Мы делаем аналог на Rust, но с тем же семантическим контрактом,
// чтобы портированные драйверы чувствовали себя как дома.
//
// Иерархия
//
//   drivers/
//     mod.rs          — этот файл: Device, Driver, DeviceManager
//     bus/
//       mod.rs        — Bus trait, BusManager
//       pci.rs        — PCI bus (сканирование, конфигурация)
//     linux_shim/
//       mod.rs        — совместимость с Linux kernel API
// =============================================================================

pub mod bus;
pub mod linux_shim;

// ---- Идентификаторы ----

/// Уникальный ID устройства в системе.
pub type DeviceId = u32;

/// Счётчик для генерации уникальных DeviceId.
static mut NEXT_DEVICE_ID: DeviceId = 1;

fn alloc_device_id() -> DeviceId {
    unsafe {
        let id = NEXT_DEVICE_ID;
        NEXT_DEVICE_ID += 1;
        id
    }
}

// ---- Класс устройства ----

/// Класс устройства — к какой категории оно относится.
/// Используется для организации устройств в /sys/class/ (как в Linux).
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u16)]
pub enum DeviceClass {
    Unknown    = 0,
    Display    = 1,   // GPU, видеокарты
    Network    = 2,   // Сетевые адаптеры
    Storage    = 3,   // Диски, NVMe, SATA
    Input      = 4,   // Клавиатуры, мыши
    Audio      = 5,   // Звуковые карты
    Usb        = 6,   // USB контроллеры
    Serial     = 7,   // Serial/UART
    Platform   = 8,   // Платформенные устройства
}

// ---- Тип шины ----

/// Тип шины, на которой обнаружено устройство.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum BusType {
    Platform = 0,   // Виртуальные / встроенные устройства
    Pci      = 1,   // PCI / PCIe
    Usb      = 2,   // USB (будущее)
    Virtio   = 3,   // VirtIO (QEMU)
}

// ---- Устройство ----

/// Устройство — аналог struct device в Linux.
///
/// Представляет физическое или виртуальное устройство в системе.
/// Содержит идентификацию, привязку к шине и состояние.
pub struct Device {
    /// Уникальный ID устройства в системе.
    pub id: DeviceId,

    /// Имя устройства (для отладки, аналог dev_name()).
    pub name: [u8; 32],
    pub name_len: usize,

    /// Класс устройства.
    pub class: DeviceClass,

    /// Шина, на которой обнаружено.
    pub bus: BusType,

    /// Vendor:Device ID (для PCI), или 0 для platform.
    pub vendor_id: u16,
    pub device_id: u16,

    /// Индекс драйвера, который обслуживает это устройство (-1 = нет).
    pub driver_index: i32,

    /// Активно ли устройство.
    pub active: bool,

    /// MMIO базовый адрес (если есть).
    pub mmio_base: u64,
    pub mmio_size: u64,

    /// IRQ номер (0 = нет).
    pub irq: u32,
}

impl Device {
    pub const fn empty() -> Self {
        Device {
            id: 0,
            name: [0; 32],
            name_len: 0,
            class: DeviceClass::Unknown,
            bus: BusType::Platform,
            vendor_id: 0,
            device_id: 0,
            driver_index: -1,
            active: false,
            mmio_base: 0,
            mmio_size: 0,
            irq: 0,
        }
    }

    pub fn set_name(&mut self, s: &str) {
        let b = s.as_bytes();
        let len = if b.len() > 31 { 31 } else { b.len() };
        self.name[..len].copy_from_slice(&b[..len]);
        self.name[len] = 0;
        self.name_len = len;
    }
}

// ---- Драйвер ----

/// Операции драйвера — аналог struct device_driver + probe/remove.
///
/// В Linux driver.probe() вызывается когда шина нашла подходящее устройство.
/// driver.remove() — когда устройство отключается.
///
/// Мы используем function pointers, потому что в no_std нет dyn trait objects
/// с аллокатором (и не хотим его тут).
pub struct DriverOps {
    /// Инициализация устройства. Возвращает 0 = OK, иначе код ошибки.
    pub probe: fn(dev: &mut Device) -> i32,

    /// Деинициализация устройства.
    pub remove: fn(dev: &mut Device),
}

/// Зарегистрированный драйвер.
pub struct Driver {
    /// Имя драйвера.
    pub name: [u8; 32],
    pub name_len: usize,

    /// Тип шины, с которой работает драйвер.
    pub bus: BusType,

    /// Vendor:Device ID, которые этот драйвер поддерживает.
    /// 0xFFFF = wildcard (любой).
    pub match_vendor: u16,
    pub match_device: u16,

    /// Операции.
    pub ops: DriverOps,

    /// Активен ли драйвер.
    pub active: bool,
}

impl Driver {
    pub const fn empty() -> Self {
        Driver {
            name: [0; 32],
            name_len: 0,
            bus: BusType::Platform,
            match_vendor: 0,
            match_device: 0,
            ops: DriverOps {
                probe: dummy_probe,
                remove: dummy_remove,
            },
            active: false,
        }
    }

    pub fn set_name(&mut self, s: &str) {
        let b = s.as_bytes();
        let len = if b.len() > 31 { 31 } else { b.len() };
        self.name[..len].copy_from_slice(&b[..len]);
        self.name[len] = 0;
        self.name_len = len;
    }

    /// Проверить, подходит ли драйвер к устройству.
    pub fn matches(&self, dev: &Device) -> bool {
        if self.bus != dev.bus {
            return false;
        }
        // Wildcard = подходит к любому
        if self.match_vendor != 0xFFFF && self.match_vendor != dev.vendor_id {
            return false;
        }
        if self.match_device != 0xFFFF && self.match_device != dev.device_id {
            return false;
        }
        true
    }
}

fn dummy_probe(_dev: &mut Device) -> i32 { -1 }
fn dummy_remove(_dev: &mut Device) {}

// ---- Глобальный менеджер устройств ----

const MAX_DEVICES: usize = 64;
const MAX_DRIVERS: usize = 32;

static mut DEVICES: [Device; MAX_DEVICES] = [const { Device::empty() }; MAX_DEVICES];
static mut DRIVERS: [Driver; MAX_DRIVERS] = [const { Driver::empty() }; MAX_DRIVERS];

/// Зарегистрировать устройство в системе.
/// Возвращает индекс устройства или None.
pub fn register_device(mut dev: Device) -> Option<usize> {
    dev.id = alloc_device_id();
    dev.active = true;

    unsafe {
        for i in 0..MAX_DEVICES {
            if !DEVICES[i].active {
                DEVICES[i] = dev;
                // Попробовать найти драйвер для устройства
                try_bind_driver(i);
                return Some(i);
            }
        }
    }
    None
}

/// Зарегистрировать драйвер.
/// После регистрации пытается привязать ко всем подходящим устройствам.
pub fn register_driver(drv: Driver) -> Option<usize> {
    unsafe {
        for i in 0..MAX_DRIVERS {
            if !DRIVERS[i].active {
                DRIVERS[i] = drv;
                DRIVERS[i].active = true;
                // Привязать ко всем подходящим устройствам без драйвера
                for d in 0..MAX_DEVICES {
                    if DEVICES[d].active && DEVICES[d].driver_index < 0
                        && DRIVERS[i].matches(&DEVICES[d])
                    {
                        let rc = (DRIVERS[i].ops.probe)(&mut DEVICES[d]);
                        if rc == 0 {
                            DEVICES[d].driver_index = i as i32;
                        }
                    }
                }
                return Some(i);
            }
        }
    }
    None
}

/// Попытаться привязать драйвер к устройству.
fn try_bind_driver(dev_idx: usize) {
    unsafe {
        for i in 0..MAX_DRIVERS {
            if DRIVERS[i].active && DRIVERS[i].matches(&DEVICES[dev_idx]) {
                let rc = (DRIVERS[i].ops.probe)(&mut DEVICES[dev_idx]);
                if rc == 0 {
                    DEVICES[dev_idx].driver_index = i as i32;
                    return;
                }
            }
        }
    }
}

/// Получить ссылку на устройство по индексу.
pub fn get_device(idx: usize) -> Option<&'static Device> {
    unsafe {
        if idx < MAX_DEVICES && DEVICES[idx].active {
            Some(&DEVICES[idx])
        } else {
            None
        }
    }
}

/// Получить мутабельную ссылку на устройство.
pub fn get_device_mut(idx: usize) -> Option<&'static mut Device> {
    unsafe {
        if idx < MAX_DEVICES && DEVICES[idx].active {
            Some(&mut DEVICES[idx])
        } else {
            None
        }
    }
}

/// Количество активных устройств.
pub fn device_count() -> usize {
    unsafe {
        let mut count = 0;
        for i in 0..MAX_DEVICES {
            if DEVICES[i].active { count += 1; }
        }
        count
    }
}

/// Количество активных драйверов.
pub fn driver_count() -> usize {
    unsafe {
        let mut count = 0;
        for i in 0..MAX_DRIVERS {
            if DRIVERS[i].active { count += 1; }
        }
        count
    }
}
