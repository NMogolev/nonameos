// =============================================================================
// NoNameOS — Подсистема задач (процессы и потоки)
// =============================================================================
//
// В микроядре задача (Task) — это единица исполнения. Бывают двух видов:
//
//   ПРОЦЕСС (Process):
//     - Имеет своё адресное пространство (свою таблицу страниц / CR3)
//     - Содержит один или более потоков
//     - Изолирован от других процессов
//     - Имеет PID (Process ID)
//
//   ПОТОК (Thread):
//     - Живёт внутри процесса, разделяет его адресное пространство
//     - Имеет свой стек и набор регистров (контекст)
//     - Планировщик переключает именно потоки
//     - Имеет TID (Thread ID)
//
// Жизненный цикл потока:
//
//   Created → Ready → Running → (Blocked | Ready) → ... → Dead
//
//   Created:  только что создан, ещё не запущен
//   Ready:    готов к выполнению, ждёт своей очереди в планировщике
//   Running:  сейчас выполняется на CPU
//   Blocked:  ждёт события (I/O, IPC, sleep, мьютекс)
//   Dead:     завершён, ресурсы ожидают освобождения
//
// Контекст-свитч (переключение потоков):
//   Когда планировщик решает переключить потоки:
//   1. Сохраняем регистры текущего потока (RSP, RIP, и т.д.)
//   2. Загружаем регистры нового потока
//   3. Если потоки в разных процессах — меняем CR3
//   4. Возвращаемся к выполнению нового потока
//
// Приоритеты:
//   0 — Idle (фоновый, когда нечего делать)
//   1 — Low (фоновые задачи)
//   2 — Normal (обычные приложения)
//   3 — High (системные сервисы)
//   4 — Realtime (драйверы, критичные задачи)
//
// В будущем планировщик будет использовать Round-Robin внутри каждого
// приоритета + aging (повышение приоритета задач, которые долго ждут).
// =============================================================================

/// Уникальный идентификатор процесса.
pub type Pid = u64;

/// Уникальный идентификатор потока.
pub type Tid = u64;

/// Состояние потока.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskState {
    Created,    // Создан, ещё не запускался
    Ready,      // Готов к выполнению
    Running,    // Выполняется прямо сейчас
    Blocked,    // Заблокирован (ждёт I/O, IPC, sleep...)
    Dead,       // Завершён
}

/// Приоритет задачи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Priority {
    Idle     = 0,
    Low      = 1,
    Normal   = 2,
    High     = 3,
    Realtime = 4,
}

/// Контекст CPU — регистры, которые сохраняются при переключении потоков.
///
/// При контекст-свитче мы сохраняем только callee-saved регистры
/// (по System V AMD64 ABI: RBX, RBP, R12-R15, RSP, RIP).
/// Caller-saved регистры (RAX, RCX, RDX, RSI, RDI, R8-R11) уже сохранены
/// вызывающим кодом по конвенции.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CpuContext {
    pub rsp: u64,   // Stack Pointer
    pub rbp: u64,   // Base Pointer (frame pointer)
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,   // Instruction Pointer (куда вернуться)
    pub rflags: u64,
}

impl CpuContext {
    pub const fn empty() -> Self {
        CpuContext {
            rsp: 0, rbp: 0, rbx: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, rflags: 0x200, // IF=1 (прерывания включены)
        }
    }
}

/// Поток (Thread) — единица планирования.
pub struct Thread {
    pub tid: Tid,               // ID потока
    pub pid: Pid,               // К какому процессу принадлежит
    pub state: TaskState,       // Текущее состояние
    pub priority: Priority,     // Приоритет
    pub context: CpuContext,    // Сохранённые регистры
    pub kernel_stack: u64,      // Адрес вершины стека ядра для этого потока
    pub name: [u8; 32],         // Имя потока (для отладки)
}

impl Thread {
    pub fn new(tid: Tid, pid: Pid, entry_point: u64, stack_top: u64) -> Self {
        let mut ctx = CpuContext::empty();
        ctx.rip = entry_point;  // откуда начать выполнение
        ctx.rsp = stack_top;    // стек растёт вниз, указываем верхушку

        Thread {
            tid,
            pid,
            state: TaskState::Created,
            priority: Priority::Normal,
            context: ctx,
            kernel_stack: 0,
            name: [0; 32],
        }
    }

    /// Установить имя потока (для отладки).
    pub fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = core::cmp::min(bytes.len(), 31);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }
}

/// Процесс — контейнер для потоков с изолированным адресным пространством.
pub struct Process {
    pub pid: Pid,
    pub cr3: u64,           // Физический адрес PML4 (адресное пространство)
    pub state: TaskState,
    pub name: [u8; 64],     // Имя процесса (для отладки)
    pub thread_count: usize,
}

impl Process {
    pub fn new(pid: Pid, cr3: u64) -> Self {
        Process {
            pid,
            cr3,
            state: TaskState::Created,
            name: [0; 64],
            thread_count: 0,
        }
    }

    pub fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = core::cmp::min(bytes.len(), 63);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }
}

// =============================================================================
// Простой планировщик (заглушка)
// =============================================================================
//
// Полноценный планировщик будет позже. Сейчас — структуры и интерфейс.
//
// Алгоритм (будущий):
//   1. Берём поток с наивысшим приоритетом из очереди Ready
//   2. Переключаем контекст: сохраняем текущий → загружаем новый
//   3. Если потоки в разных процессах — меняем CR3 (адресное пространство)
//   4. Возвращаемся к выполнению
//
// Планировщик вызывается:
//   - По таймеру (IRQ 0) — preemptive multitasking
//   - При блокировке потока (I/O, IPC) — yield
//   - При завершении потока — exit
// =============================================================================

/// Счётчик для генерации уникальных PID/TID.
static mut NEXT_PID: Pid = 1;
static mut NEXT_TID: Tid = 1;

/// Выделить новый уникальный PID.
pub fn alloc_pid() -> Pid {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}

/// Выделить новый уникальный TID.
pub fn alloc_tid() -> Tid {
    unsafe {
        let tid = NEXT_TID;
        NEXT_TID += 1;
        tid
    }
}
