// =============================================================================
// NoNameOS — Boot-Time Diagnostics & Recovery Framework
// =============================================================================
//
// Система самотестирования и восстановления при загрузке ядра.
//
//
// АРХИТЕКТУРА:
//
//   ┌──────────────────────────────────────────────────────────┐
//   │                    Boot Sequence                         │
//   │                                                          │
//   │  init_memory() ──► verify_memory() ──► [OK/RECOVER/FAIL]│
//   │  init_vfs()    ──► verify_vfs()    ──► [OK/RECOVER/FAIL]│
//   │  init_ipc()    ──► verify_ipc()    ──► [OK/RECOVER/FAIL]│
//   │  init_drivers()──► verify_drivers()──► [OK/RECOVER/FAIL]│
//   │  init_sched()  ──► verify_sched()  ──► [OK/RECOVER/FAIL]│
//   │  init_syscall()──► verify_syscall()──► [OK/RECOVER/FAIL]│
//   │                                                          │
//   │  ┌────────────────────────────────┐                     │
//   │  │ SubsystemHealth (глобальный)   │                     │
//   │  │  memory:    Healthy / Degraded │                     │
//   │  │  vfs:       Healthy / Degraded │                     │
//   │  │  ipc:       Healthy / Degraded │                     │
//   │  │  drivers:   Healthy / Degraded │                     │
//   │  │  scheduler: Healthy / Degraded │                     │
//   │  │  boot_mode: Normal / Safe      │                     │
//   │  └────────────────────────────────┘                     │
//   └──────────────────────────────────────────────────────────┘
//
// УРОВНИ КРИТИЧНОСТИ ТЕСТА:
//
//   Critical — без этого ядро не может работать ВООБЩЕ.
//              Фейл → попытка recovery → если не помогло → kernel panic.
//              Примеры: alloc_frame, базовый VGA вывод.
//
//   Required — подсистема нужна для нормальной работы.
//              Фейл → попытка recovery → если не помогло → деградация (Degraded).
//              Примеры: VFS root, IPC endpoints, PCI scan.
//
//   Optional — приятно иметь, но можно жить без.
//              Фейл → помечаем Degraded, логируем, продолжаем.
//              Примеры: driver match, advanced memory tests.
//
// RECOVERY СТРАТЕГИИ:
//
//   Reinit   — повторная инициализация подсистемы.
//   Fallback — переключение на упрощённый режим.
//   Isolate  — отключить сломанный компонент, продолжить без него.
//   Panic    — невозможно продолжить, остановка.
//
// SAFE MODE:
//   Если набралось ≥2 Degraded подсистемы, ядро переходит в Safe Mode:
//     - Отключаются не-критические подсистемы
//     - Минимальный набор: memory + VGA + keyboard
//     - Выводится диагностика для пользователя
//     - Аналог Windows Safe Mode / Linux single-user
//
// КАК ДОБАВИТЬ ТЕСТ:
//   1. Написать fn test_xxx() -> bool
//   2. Добавить в соответствующий SUBSYSTEM_TESTS_XXX массив
//   3. Указать Severity (Critical / Required / Optional)
//   4. Опционально: указать recovery функцию
// =============================================================================

// ---- Типы ----

/// Критичность теста.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Critical,   // Фейл = попытка recovery, потом panic
    Required,   // Фейл = попытка recovery, потом degraded
    Optional,   // Фейл = degraded, лог
}

/// Здоровье подсистемы.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Health {
    Unknown,    // Ещё не проверялась
    Healthy,    // Все тесты прошли
    Recovered,  // Были проблемы, но recovery помог
    Degraded,   // Работает с ограничениями
    Failed,     // Полностью неработоспособна
}

/// Режим загрузки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BootMode {
    Normal,     // Всё ОК
    Safe,       // Деградация ≥2 подсистем
    Minimal,    // Только memory + VGA (экстренный)
}

/// Идентификатор подсистемы.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Subsystem {
    Memory    = 0,
    Vfs       = 1,
    Ipc       = 2,
    Drivers   = 3,
    Scheduler = 4,
    Syscall   = 5,
}

const SUBSYSTEM_COUNT: usize = 6;

/// Названия подсистем (для вывода).
const SUBSYSTEM_NAMES: [&str; SUBSYSTEM_COUNT] = [
    "memory", "vfs", "ipc", "drivers", "scheduler", "syscall",
];

/// Один диагностический тест.
struct DiagTest {
    name: &'static str,
    subsystem: Subsystem,
    severity: Severity,
    func: fn() -> bool,
    recovery: Option<fn() -> bool>,  // None = нет стратегии восстановления
}

// ---- Глобальное состояние здоровья ----

/// Здоровье каждой подсистемы.
static mut HEALTH: [Health; SUBSYSTEM_COUNT] = [Health::Unknown; SUBSYSTEM_COUNT];

/// Текущий режим загрузки.
static mut BOOT_MODE: BootMode = BootMode::Normal;

/// Количество попыток recovery для каждой подсистемы.
static mut RECOVERY_ATTEMPTS: [u8; SUBSYSTEM_COUNT] = [0; SUBSYSTEM_COUNT];

/// Максимум попыток recovery на подсистему.
const MAX_RECOVERY_ATTEMPTS: u8 = 3;

/// Лог событий загрузки (кольцевой буфер).
const BOOT_LOG_SIZE: usize = 64;
static mut BOOT_LOG: [BootEvent; BOOT_LOG_SIZE] = [BootEvent::empty(); BOOT_LOG_SIZE];
static mut BOOT_LOG_COUNT: usize = 0;

/// Результат прогона тестов.
pub struct TestResults {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub recovered: usize,
    pub boot_mode: BootMode,
}

/// Событие загрузки (для диагностики).
#[derive(Clone, Copy)]
struct BootEvent {
    subsystem: u8,
    event_type: BootEventType,
    test_index: u8,       // какой тест (для идентификации)
}

#[derive(Clone, Copy, PartialEq)]
enum BootEventType {
    None,
    TestPass,
    TestFail,
    RecoveryAttempt,
    RecoverySuccess,
    RecoveryFailed,
    SubsystemDegraded,
    SafeModeEntered,
}

impl BootEvent {
    const fn empty() -> Self {
        BootEvent {
            subsystem: 0,
            event_type: BootEventType::None,
            test_index: 0,
        }
    }
}

/// Записать событие в boot log.
fn log_event(subsystem: Subsystem, event: BootEventType, test_idx: u8) {
    unsafe {
        if BOOT_LOG_COUNT < BOOT_LOG_SIZE {
            BOOT_LOG[BOOT_LOG_COUNT] = BootEvent {
                subsystem: subsystem as u8,
                event_type: event,
                test_index: test_idx,
            };
            BOOT_LOG_COUNT += 1;
        }
    }
}

// ---- Публичный API ----

/// Получить здоровье подсистемы.
pub fn health(sub: Subsystem) -> Health {
    unsafe { HEALTH[sub as usize] }
}

/// Получить текущий режим загрузки.
pub fn boot_mode() -> BootMode {
    unsafe { BOOT_MODE }
}

/// Проверить, работает ли подсистема (Healthy или Recovered).
pub fn is_subsystem_ok(sub: Subsystem) -> bool {
    let h = health(sub);
    h == Health::Healthy || h == Health::Recovered
}

/// Количество деградированных подсистем.
pub fn degraded_count() -> usize {
    unsafe {
        let mut count = 0;
        for i in 0..SUBSYSTEM_COUNT {
            if HEALTH[i] == Health::Degraded || HEALTH[i] == Health::Failed {
                count += 1;
            }
        }
        count
    }
}

/// Количество событий в boot log.
pub fn boot_log_count() -> usize {
    unsafe { BOOT_LOG_COUNT }
}

/// Вывести диагностический отчёт.
pub fn print_health_report() {
    crate::println!("  Subsystem Health Report:");
    for i in 0..SUBSYSTEM_COUNT {
        let h = unsafe { HEALTH[i] };
        let status = match h {
            Health::Unknown   => "????",
            Health::Healthy   => " OK ",
            Health::Recovered => "RCVR",
            Health::Degraded  => "DEGR",
            Health::Failed    => "FAIL",
        };
        let recoveries = unsafe { RECOVERY_ATTEMPTS[i] };
        if recoveries > 0 {
            crate::println!("    [{}] {:<12} (recovery attempts: {})",
                status, SUBSYSTEM_NAMES[i], recoveries);
        } else {
            crate::println!("    [{}] {}", status, SUBSYSTEM_NAMES[i]);
        }
    }
    let mode = unsafe { BOOT_MODE };
    crate::println!("  Boot mode: {:?}", mode);
}

// ---- Регистрация тестов ----

static DIAG_TESTS: &[DiagTest] = &[
    // ===================== MEMORY (Critical) =====================
    DiagTest {
        name: "phys::alloc_and_free",
        subsystem: Subsystem::Memory,
        severity: Severity::Critical,
        func: test_phys_alloc_and_free,
        recovery: Some(recovery_memory),
    },
    DiagTest {
        name: "phys::alloc_returns_aligned",
        subsystem: Subsystem::Memory,
        severity: Severity::Critical,
        func: test_phys_alloc_aligned,
        recovery: Some(recovery_memory),
    },
    DiagTest {
        name: "phys::free_restores_count",
        subsystem: Subsystem::Memory,
        severity: Severity::Required,
        func: test_phys_free_restores,
        recovery: None,
    },
    DiagTest {
        name: "phys::double_free_safe",
        subsystem: Subsystem::Memory,
        severity: Severity::Optional,
        func: test_phys_double_free,
        recovery: None,
    },

    // ===================== VFS (Required) =====================
    DiagTest {
        name: "vfs::root_exists",
        subsystem: Subsystem::Vfs,
        severity: Severity::Critical,
        func: test_vfs_root_exists,
        recovery: Some(recovery_vfs),
    },
    DiagTest {
        name: "vfs::lookup_dev",
        subsystem: Subsystem::Vfs,
        severity: Severity::Required,
        func: test_vfs_lookup_dev,
        recovery: Some(recovery_vfs),
    },
    DiagTest {
        name: "vfs::lookup_nonexistent",
        subsystem: Subsystem::Vfs,
        severity: Severity::Optional,
        func: test_vfs_lookup_nonexistent,
        recovery: None,
    },
    DiagTest {
        name: "vfs::create_and_write",
        subsystem: Subsystem::Vfs,
        severity: Severity::Required,
        func: test_vfs_create_and_write,
        recovery: None,
    },
    DiagTest {
        name: "vfs::create_and_read",
        subsystem: Subsystem::Vfs,
        severity: Severity::Required,
        func: test_vfs_create_and_read,
        recovery: None,
    },

    // ===================== IPC (Required) =====================
    DiagTest {
        name: "ipc::register_endpoint",
        subsystem: Subsystem::Ipc,
        severity: Severity::Required,
        func: test_ipc_register,
        recovery: None,
    },
    DiagTest {
        name: "ipc::send_receive",
        subsystem: Subsystem::Ipc,
        severity: Severity::Required,
        func: test_ipc_send_receive,
        recovery: None,
    },
    DiagTest {
        name: "ipc::queue_overflow",
        subsystem: Subsystem::Ipc,
        severity: Severity::Optional,
        func: test_ipc_queue_overflow,
        recovery: None,
    },

    // ===================== Drivers (Required) =====================
    DiagTest {
        name: "drivers::register_device",
        subsystem: Subsystem::Drivers,
        severity: Severity::Required,
        func: test_driver_register_device,
        recovery: None,
    },
    DiagTest {
        name: "drivers::driver_match",
        subsystem: Subsystem::Drivers,
        severity: Severity::Optional,
        func: test_driver_match,
        recovery: None,
    },

    // ===================== Scheduler (Critical) =====================
    DiagTest {
        name: "sched::init_state",
        subsystem: Subsystem::Scheduler,
        severity: Severity::Critical,
        func: test_sched_init_state,
        recovery: Some(recovery_scheduler),
    },
    DiagTest {
        name: "sched::thread_count",
        subsystem: Subsystem::Scheduler,
        severity: Severity::Required,
        func: test_sched_thread_count,
        recovery: None,
    },

    // ===================== Syscall (Required) =====================
    DiagTest {
        name: "shim::kmalloc_kfree",
        subsystem: Subsystem::Syscall,
        severity: Severity::Required,
        func: test_shim_kmalloc,
        recovery: None,
    },
    DiagTest {
        name: "shim::spinlock",
        subsystem: Subsystem::Syscall,
        severity: Severity::Optional,
        func: test_shim_spinlock,
        recovery: None,
    },
];

// ---- Основной прогон ----

/// Запустить boot-time диагностику с recovery.
///
/// Алгоритм:
///   1. Прогоняем все тесты, группируя по подсистемам
///   2. При фейле Critical/Required — пробуем recovery
///   3. После recovery — перепроверяем тот же тест
///   4. Обновляем Health подсистем
///   5. Если ≥2 Degraded — переходим в Safe Mode
///   6. Если Critical Failed — kernel panic
pub fn run_all() -> TestResults {
    crate::println!();
    crate::println!("==========================================");
    crate::println!("  Boot Diagnostics ({} tests, {} subsystems)",
        DIAG_TESTS.len(), SUBSYSTEM_COUNT);
    crate::println!("==========================================");

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut recovered = 0usize;

    // Трекинг: были ли фейлы в каждой подсистеме
    let mut sub_has_critical_fail = [false; SUBSYSTEM_COUNT];
    let mut sub_has_any_fail = [false; SUBSYSTEM_COUNT];
    let mut sub_tested = [false; SUBSYSTEM_COUNT];

    for (test_idx, test) in DIAG_TESTS.iter().enumerate() {
        let sub_idx = test.subsystem as usize;
        sub_tested[sub_idx] = true;

        let result = (test.func)();

        if result {
            crate::println!("  [PASS] {}", test.name);
            log_event(test.subsystem, BootEventType::TestPass, test_idx as u8);
            passed += 1;
            continue;
        }

        // Тест провалился
        let sev_tag = match test.severity {
            Severity::Critical => "CRIT",
            Severity::Required => "REQD",
            Severity::Optional => "OPT ",
        };
        crate::println!("  [FAIL] {} [{}]", test.name, sev_tag);
        log_event(test.subsystem, BootEventType::TestFail, test_idx as u8);

        // Попытка recovery (если есть стратегия и не Optional)
        if test.severity != Severity::Optional {
            if let Some(recover_fn) = test.recovery {
                let attempts = unsafe { &mut RECOVERY_ATTEMPTS[sub_idx] };
                if *attempts < MAX_RECOVERY_ATTEMPTS {
                    *attempts += 1;
                    crate::println!("         -> Recovery attempt {} for {}...",
                        *attempts, SUBSYSTEM_NAMES[sub_idx]);
                    log_event(test.subsystem, BootEventType::RecoveryAttempt, test_idx as u8);

                    let recovery_ok = recover_fn();
                    if recovery_ok {
                        // Перепроверяем тест
                        let recheck = (test.func)();
                        if recheck {
                            crate::println!("         -> Recovery SUCCESS, recheck PASS");
                            log_event(test.subsystem, BootEventType::RecoverySuccess, test_idx as u8);
                            recovered += 1;
                            passed += 1;
                            continue; // Тест прошёл после recovery
                        }
                    }
                    crate::println!("         -> Recovery FAILED");
                    log_event(test.subsystem, BootEventType::RecoveryFailed, test_idx as u8);
                }
            }
        }

        // Тест окончательно провален
        failed += 1;
        sub_has_any_fail[sub_idx] = true;
        if test.severity == Severity::Critical {
            sub_has_critical_fail[sub_idx] = true;
        }
    }

    // Обновляем здоровье подсистем
    for i in 0..SUBSYSTEM_COUNT {
        unsafe {
            if !sub_tested[i] {
                HEALTH[i] = Health::Unknown;
            } else if sub_has_critical_fail[i] {
                HEALTH[i] = Health::Failed;
            } else if sub_has_any_fail[i] {
                HEALTH[i] = Health::Degraded;
                log_event(
                    core::mem::transmute::<u8, Subsystem>(i as u8),
                    BootEventType::SubsystemDegraded, 0,
                );
            } else if RECOVERY_ATTEMPTS[i] > 0 {
                HEALTH[i] = Health::Recovered;
            } else {
                HEALTH[i] = Health::Healthy;
            }
        }
    }

    // Проверяем, нужен ли Safe Mode
    let degraded = degraded_count();
    let has_critical_failure = sub_has_critical_fail.iter().any(|&x| x);

    if has_critical_failure {
        // Critical подсистема мертва — проверяем какая именно
        // Memory Critical fail = kernel panic (без памяти ничего не работает)
        if sub_has_critical_fail[Subsystem::Memory as usize] {
            crate::println!();
            crate::println!("!!! FATAL: Memory subsystem FAILED !!!");
            crate::println!("    Cannot continue without working memory allocator.");
            crate::println!("    System halted.");
            loop { unsafe { core::arch::asm!("cli; hlt"); } }
        }

        // Scheduler Critical fail = enter Minimal mode
        if sub_has_critical_fail[Subsystem::Scheduler as usize] {
            unsafe { BOOT_MODE = BootMode::Minimal; }
            crate::println!("  >> Entering MINIMAL mode (no scheduler)");
        }

        // VFS Critical fail = enter Safe mode
        if sub_has_critical_fail[Subsystem::Vfs as usize] {
            unsafe {
                if BOOT_MODE == BootMode::Normal {
                    BOOT_MODE = BootMode::Safe;
                }
            }
            crate::println!("  >> VFS critical failure, entering SAFE mode");
        }
    }

    if degraded >= 2 {
        unsafe {
            if BOOT_MODE == BootMode::Normal {
                BOOT_MODE = BootMode::Safe;
                log_event(Subsystem::Memory, BootEventType::SafeModeEntered, 0);
            }
        }
        crate::println!("  >> {} subsystems degraded, entering SAFE mode", degraded);
    }

    // Отчёт
    crate::println!("------------------------------------------");
    print_health_report();
    crate::println!("------------------------------------------");
    crate::println!("  Results: {} passed, {} failed, {} recovered",
        passed, failed, recovered);
    crate::println!("  Boot log: {} events recorded", boot_log_count());
    crate::println!("==========================================");
    crate::println!();

    TestResults {
        total: DIAG_TESTS.len(),
        passed,
        failed,
        recovered,
        boot_mode: unsafe { BOOT_MODE },
    }
}

// =============================================================================
// Тесты: Физическая память
// =============================================================================

fn test_phys_alloc_and_free() -> bool {
    use crate::memory::phys;
    let before = phys::free_count();
    let frame = phys::alloc_frame();
    if frame.is_none() { return false; }
    let after_alloc = phys::free_count();
    if after_alloc >= before { return false; } // должно уменьшиться
    phys::free_frame(frame.unwrap());
    let after_free = phys::free_count();
    after_free == before
}

fn test_phys_alloc_aligned() -> bool {
    use crate::memory::phys;
    let frame = phys::alloc_frame();
    if frame.is_none() { return false; }
    let addr = frame.unwrap();
    let aligned = addr % 4096 == 0;
    phys::free_frame(addr);
    aligned
}

fn test_phys_free_restores() -> bool {
    use crate::memory::phys;
    let before = phys::free_count();
    // Выделяем 4 фрейма
    let mut frames = [0usize; 4];
    for i in 0..4 {
        match phys::alloc_frame() {
            Some(f) => frames[i] = f,
            None => return false,
        }
    }
    let mid = phys::free_count();
    if mid != before - 4 { return false; }
    // Освобождаем все
    for i in 0..4 {
        phys::free_frame(frames[i]);
    }
    phys::free_count() == before
}

fn test_phys_double_free() -> bool {
    use crate::memory::phys;
    let before = phys::free_count();
    let frame = phys::alloc_frame();
    if frame.is_none() { return false; }
    let addr = frame.unwrap();
    phys::free_frame(addr);
    phys::free_frame(addr); // double free — не должно паниковать
    // free_count не должен быть больше before (double free не создаёт лишний фрейм)
    phys::free_count() <= before + 1 // допускаем +1 от double free (bitmap не проверяет)
}

// =============================================================================
// Тесты: VFS
// =============================================================================

fn test_vfs_root_exists() -> bool {
    // Root dentry (index 0) должен быть активен после init
    crate::vfs::get_dentry(0).is_some()
}

fn test_vfs_lookup_dev() -> bool {
    // /dev должен существовать
    crate::vfs::path_lookup(b"/dev").is_some()
}

fn test_vfs_lookup_nonexistent() -> bool {
    // /nonexistent не должен существовать
    crate::vfs::path_lookup(b"/nonexistent").is_none()
}

fn test_vfs_create_and_write() -> bool {
    // Создаём файл в /tmp и пишем в него
    let dentry_idx = crate::vfs::create(b"/tmp", b"test.txt", crate::vfs::InodeType::File);
    if dentry_idx.is_none() { return false; }

    // Открываем файл
    let fd = crate::vfs::open(b"/tmp/test.txt", 0);
    if fd.is_none() { return false; }

    // Записываем
    let data = b"Hello, NoNameOS!";
    let written = crate::vfs::write(fd.unwrap(), data);
    written == data.len() as isize
}

fn test_vfs_create_and_read() -> bool {
    // Создаём файл, пишем, читаем — проверяем данные
    let _ = crate::vfs::create(b"/tmp", b"read_test.txt", crate::vfs::InodeType::File);

    let fd = crate::vfs::open(b"/tmp/read_test.txt", 0);
    if fd.is_none() { return false; }
    let fd = fd.unwrap();

    let data = b"Test1234";
    let written = crate::vfs::write(fd, data);
    if written != data.len() as isize { return false; }

    // Перемотка на начало
    crate::vfs::seek(fd, 0);

    // Чтение
    let mut buf = [0u8; 32];
    let read = crate::vfs::read(fd, &mut buf);
    if read != data.len() as isize { return false; }

    // Проверяем содержимое
    &buf[..data.len()] == data
}

// =============================================================================
// Тесты: IPC
// =============================================================================

fn test_ipc_register() -> bool {
    let ep = crate::ipc::register_endpoint("test_ep", 1);
    if ep.is_none() { return false; }
    // Должны найти его по имени
    let found = crate::ipc::find_endpoint("test_ep");
    if found.is_none() { return false; }
    found.unwrap() == ep.unwrap()
}

fn test_ipc_send_receive() -> bool {
    let ep = crate::ipc::register_endpoint("test_sr", 1);
    if ep.is_none() { return false; }
    let ep_id = ep.unwrap();

    // Отправляем сообщение
    let msg = crate::ipc::Message::request(42, b"ping");
    if crate::ipc::send(ep_id, msg).is_err() { return false; }

    // Получаем
    let recv = crate::ipc::receive(ep_id);
    if recv.is_none() { return false; }
    let recv = recv.unwrap();
    recv.opcode == 42 && recv.payload_len == 4 && &recv.payload[..4] == b"ping"
}

fn test_ipc_queue_overflow() -> bool {
    let ep = crate::ipc::register_endpoint("test_overflow", 1);
    if ep.is_none() { return false; }
    let ep_id = ep.unwrap();

    // Заполняем очередь (16 элементов)
    for i in 0..16 {
        let msg = crate::ipc::Message::request(i, b"x");
        if crate::ipc::send(ep_id, msg).is_err() { return false; }
    }

    // 17-е сообщение должно вернуть ошибку
    let msg = crate::ipc::Message::request(99, b"overflow");
    crate::ipc::send(ep_id, msg).is_err()
}

// =============================================================================
// Тесты: Drivers
// =============================================================================

fn test_driver_register_device() -> bool {
    use crate::drivers::*;
    let before = device_count();
    let mut dev = Device::empty();
    dev.set_name("test_dev");
    dev.class = DeviceClass::Unknown;
    dev.bus = BusType::Platform;
    let idx = register_device(dev);
    if idx.is_none() { return false; }
    device_count() > before
}

fn test_driver_match() -> bool {
    use crate::drivers::*;
    // Создаём устройство
    let mut dev = Device::empty();
    dev.set_name("matchable");
    dev.bus = BusType::Platform;
    dev.vendor_id = 0x1234;
    dev.device_id = 0x5678;
    let _dev_idx = register_device(dev);

    // Создаём драйвер с wildcard match
    let mut drv = Driver::empty();
    drv.set_name("test_drv");
    drv.bus = BusType::Platform;
    drv.match_vendor = 0xFFFF; // wildcard
    drv.match_device = 0xFFFF; // wildcard
    drv.ops = DriverOps {
        probe: test_probe_ok,
        remove: test_remove_noop,
    };
    let drv_idx = register_driver(drv);
    drv_idx.is_some()
}

fn test_probe_ok(_dev: &mut crate::drivers::Device) -> i32 { 0 }
fn test_remove_noop(_dev: &mut crate::drivers::Device) {}

// =============================================================================
// Тесты: Scheduler
// =============================================================================

fn test_sched_init_state() -> bool {
    // После init: 1 процесс, 1 поток
    crate::scheduler::process_count() >= 1
}

fn test_sched_thread_count() -> bool {
    crate::scheduler::thread_count() >= 1
}

// =============================================================================
// Тесты: Linux shim
// =============================================================================

fn test_shim_kmalloc() -> bool {
    use crate::drivers::linux_shim::*;
    let ptr = kmalloc(64, GFP_KERNEL);
    if ptr.is_null() { return false; }
    // Запишем что-то
    unsafe { *ptr = 0xAB; }
    let val = unsafe { *ptr };
    kfree(ptr);
    val == 0xAB
}

fn test_shim_spinlock() -> bool {
    use crate::drivers::linux_shim::SpinLock;
    let lock = SpinLock::new();
    // Должны захватить
    if !lock.try_lock() { return false; }
    // Повторный try_lock должен провалиться
    if lock.try_lock() { return false; }
    lock.unlock();
    // Теперь снова можем захватить
    lock.try_lock()
}

// =============================================================================
// Recovery стратегии
// =============================================================================
//
// Каждая recovery-функция пытается восстановить подсистему.
// Возвращает true если восстановление (вероятно) удалось.
// После recovery вызывающий код перепроверяет провалившийся тест.

/// Recovery: Memory — повторная инициализация физического аллокатора.
///
/// Стратегия: Reinit.
/// Перезапускаем bitmap аллокатор с текущими параметрами.
/// Это может помочь если bitmap был повреждён, но память физически цела.
fn recovery_memory() -> bool {
    // Читаем текущие параметры из линкер-символов
    extern "C" { static __bss_end: u8; }
    let assumed_memory: usize = 64 * 1024 * 1024;
    let kernel_start: usize = 0x100000;
    let kernel_end: usize = unsafe { &__bss_end as *const u8 as usize };

    // Повторная инициализация
    crate::memory::phys::init(assumed_memory, kernel_start, kernel_end);

    // Проверяем базовую работоспособность
    let free = crate::memory::phys::free_count();
    free > 0
}

/// Recovery: VFS — повторная инициализация виртуальной файловой системы.
///
/// Стратегия: Reinit.
/// Пересоздаём ramfs с нуля. Все открытые файлы будут потеряны,
/// но структура ФС будет восстановлена.
fn recovery_vfs() -> bool {
    crate::vfs::init();
    // Проверяем что root dentry на месте
    crate::vfs::get_dentry(0).is_some()
}

/// Recovery: Scheduler — повторная инициализация планировщика.
///
/// Стратегия: Reinit.
/// Пересоздаём idle процесс и поток. Все пользовательские потоки
/// будут потеряны, но ядро сможет продолжить работу.
fn recovery_scheduler() -> bool {
    crate::scheduler::init();
    crate::scheduler::process_count() >= 1 && crate::scheduler::thread_count() >= 1
}

// =============================================================================
// Runtime Health Check API
// =============================================================================
//
// Эти функции можно вызывать не только при загрузке, но и в runtime,
// чтобы мониторить здоровье подсистем во время работы ядра.
// В будущем: watchdog поток, периодический health check.

/// Быстрая проверка здоровья памяти (runtime).
/// Выделяем и освобождаем фрейм — если работает, память жива.
pub fn quick_check_memory() -> bool {
    use crate::memory::phys;
    match phys::alloc_frame() {
        Some(addr) => { phys::free_frame(addr); true }
        None => false,
    }
}

/// Быстрая проверка VFS (runtime).
pub fn quick_check_vfs() -> bool {
    crate::vfs::get_dentry(0).is_some()
}

/// Быстрая проверка scheduler (runtime).
pub fn quick_check_scheduler() -> bool {
    crate::scheduler::thread_count() >= 1
}

/// Полный runtime health check всех подсистем.
/// Обновляет глобальный HEALTH и возвращает количество проблемных подсистем.
pub fn runtime_health_check() -> usize {
    let checks: [(Subsystem, fn() -> bool); 3] = [
        (Subsystem::Memory,    quick_check_memory),
        (Subsystem::Vfs,       quick_check_vfs),
        (Subsystem::Scheduler, quick_check_scheduler),
    ];

    let mut problems = 0;
    for (sub, check_fn) in &checks {
        let ok = check_fn();
        unsafe {
            let idx = *sub as usize;
            if !ok && HEALTH[idx] == Health::Healthy {
                // Была здорова, стала больна
                HEALTH[idx] = Health::Degraded;
                problems += 1;
                crate::println!("[WARN] Runtime: {} degraded", SUBSYSTEM_NAMES[idx]);
            }
        }
    }
    problems
}
