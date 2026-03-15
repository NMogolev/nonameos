// =============================================================================
// NoNameOS — Scheduler (Планировщик)
// =============================================================================
//
// Планировщик управляет потоками: решает кто и когда выполняется на CPU.
//
// Алгоритм: Round-Robin с приоритетами.
//   - Потоки с одинаковым приоритетом получают равные кванты времени
//   - Потоки с более высоким приоритетом вытесняют потоки с низким
//   - Вызывается по IRQ 0 (таймер) — preemptive multitasking
//   - Вызывается при yield/block/exit — cooperative
//
// Контекст-свитч:
//   1. Сохраняем RSP текущего потока
//   2. Выбираем следующий Ready-поток
//   3. Восстанавливаем RSP нового потока
//   4. Если потоки в разных процессах — меняем CR3
//   5. ret → CPU начинает исполнять новый поток
//
// Взаимодействие с остальными подсистемами:
//   - task.rs      — структуры Thread/Process
//   - idt.rs       — IRQ 0 (timer) вызывает schedule()
//   - syscall.rs   — sys_yield(), sys_exit()
//   - ipc.rs       — блокировка потоков при ожидании сообщений
//
// Текущая реализация: массив потоков фиксированного размера.
// В будущем: linked list очередей по приоритетам.
// =============================================================================

use crate::task::*;
use crate::memory::phys;

// ---- Константы ----

const MAX_THREADS: usize = 64;
const MAX_PROCESSES: usize = 16;

/// Размер стека ядра для каждого потока (16 KiB).
const KERNEL_STACK_SIZE: usize = 4 * 4096;

// ---- Глобальное состояние ----

/// Таблица потоков.
static mut THREADS: [Option<Thread>; MAX_THREADS] = [const { None }; MAX_THREADS];

/// Таблица процессов.
static mut PROCESSES: [Option<Process>; MAX_PROCESSES] = [const { None }; MAX_PROCESSES];

/// Индекс текущего выполняющегося потока (-1 = нет).
static mut CURRENT_THREAD: i32 = -1;

/// Счётчик тиков таймера (для статистики и sleep).
static mut TICK_COUNT: u64 = 0;

/// Планировщик включён (false до первого schedule()).
static mut SCHEDULER_ACTIVE: bool = false;

/// Таймер пробуждения: если > 0, поток спит до этого тика.
static mut SLEEP_UNTIL: [u64; MAX_THREADS] = [0; MAX_THREADS];

// ---- Инициализация ----

/// Инициализация планировщика.
///
/// Создаёт kernel idle процесс (PID 0) с одним idle потоком.
/// Idle поток выполняется когда нет других Ready потоков.
pub fn init() {
    // Создаём процесс ядра (PID 0)
    let pid = alloc_pid();
    let mut proc = Process::new(pid, 0); // CR3 = 0, используем текущие таблицы
    proc.set_name("kernel");
    proc.state = TaskState::Running;
    proc.thread_count = 1;

    unsafe {
        PROCESSES[0] = Some(proc);
    }

    // Создаём kernel idle поток (TID 0)
    // Entry point и стек не важны — мы "становимся" этим потоком прямо сейчас.
    let tid = alloc_tid();
    let mut thread = Thread::new(tid, pid, 0, 0);
    thread.set_name("idle");
    thread.state = TaskState::Running;
    thread.priority = Priority::Idle;

    unsafe {
        THREADS[0] = Some(thread);
        CURRENT_THREAD = 0;
        SCHEDULER_ACTIVE = true;
    }
}

// ---- Создание потоков и процессов ----

/// Создать новый kernel-mode поток.
///
/// `entry` — адрес функции (fn() -> !) которую поток будет выполнять.
/// `name` — имя для отладки.
///
/// Возвращает TID или None если нет места.
pub fn spawn_kernel_thread(entry: fn() -> !, name: &str) -> Option<Tid> {
    let pid = 0; // kernel process

    // Выделяем стек для потока
    let stack_bottom = alloc_kernel_stack()?;
    let stack_top = stack_bottom + KERNEL_STACK_SIZE;

    let tid = alloc_tid();
    let mut thread = Thread::new(tid, pid, entry as u64, stack_top as u64);
    thread.set_name(name);
    thread.state = TaskState::Ready;
    thread.kernel_stack = stack_top as u64;

    // Подготавливаем стек так, чтобы context_switch мог "вернуться" на entry.
    // Кладём на стек начальный контекст: callee-saved регистры + return address.
    // При первом switch_to: восстановятся регистры, ret прыгнет на entry.
    unsafe {
        let sp = stack_top as *mut u64;
        // Стек растёт вниз. Кладём в порядке, обратном pop в switch_to.
        // switch_to делает: pop r15, pop r14, pop r13, pop r12, pop rbx, pop rbp, ret
        // Значит кладём: rbp, rbx, r12, r13, r14, r15 (снизу вверх), потом return addr
        let sp = sp.sub(1); *sp = entry as u64;  // return address (ret прыгнет сюда)
        let sp = sp.sub(1); *sp = 0;             // rbp
        let sp = sp.sub(1); *sp = 0;             // rbx
        let sp = sp.sub(1); *sp = 0;             // r12
        let sp = sp.sub(1); *sp = 0;             // r13
        let sp = sp.sub(1); *sp = 0;             // r14
        let sp = sp.sub(1); *sp = 0;             // r15

        thread.context.rsp = sp as u64;
    }

    // Находим свободный слот
    unsafe {
        for i in 0..MAX_THREADS {
            if THREADS[i].is_none() {
                THREADS[i] = Some(thread);
                // Увеличиваем счётчик потоков в процессе
                if let Some(ref mut p) = PROCESSES[0] {
                    p.thread_count += 1;
                }
                return Some(tid);
            }
        }
    }
    None
}

/// Выделить стек ядра (KERNEL_STACK_SIZE байт).
/// Возвращает адрес дна стека.
fn alloc_kernel_stack() -> Option<usize> {
    let pages = KERNEL_STACK_SIZE / 4096;
    let mut base = 0;
    for i in 0..pages {
        match phys::alloc_frame() {
            Some(addr) => {
                if i == 0 { base = addr; }
                // Обнуляем
                unsafe { core::ptr::write_bytes(addr as *mut u8, 0, 4096); }
            }
            None => return None,
        }
    }
    Some(base)
}

// ---- Контекст-свитч (asm) ----
//
// switch_to(old_rsp: &mut u64, new_rsp: u64)
//
// Сохраняет callee-saved регистры на текущий стек,
// сохраняет RSP в *old_rsp, загружает new_rsp в RSP,
// восстанавливает callee-saved регистры с нового стека,
// ret → прыгает на return address нового потока.

core::arch::global_asm!(r#"
.global switch_to
switch_to:
    // Сохраняем callee-saved регистры текущего потока
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15

    // Сохраняем текущий RSP в *old_rsp (RDI = &mut old_rsp)
    mov [rdi], rsp

    // Загружаем RSP нового потока (RSI = new_rsp)
    mov rsp, rsi

    // Восстанавливаем callee-saved регистры нового потока
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp

    // ret прыгнет на адрес, который лежит на вершине стека нового потока.
    // Для нового потока это entry_point, для старого — точка после call switch_to.
    ret
"#);

extern "C" {
    fn switch_to(old_rsp: *mut u64, new_rsp: u64);
}

// ---- Планирование ----

/// Создать kernel-mode поток с явным приоритетом.
pub fn spawn_kernel_thread_with_priority(
    entry: fn() -> !,
    name: &str,
    priority: Priority,
) -> Option<Tid> {
    let pid = 0;
    let stack_bottom = alloc_kernel_stack()?;
    let stack_top = stack_bottom + KERNEL_STACK_SIZE;

    let tid = alloc_tid();
    let mut thread = Thread::new(tid, pid, entry as u64, stack_top as u64);
    thread.set_name(name);
    thread.state = TaskState::Ready;
    thread.priority = priority;
    thread.kernel_stack = stack_top as u64;

    unsafe {
        let sp = stack_top as *mut u64;
        let sp = sp.sub(1); *sp = entry as u64;
        let sp = sp.sub(1); *sp = 0; // rbp
        let sp = sp.sub(1); *sp = 0; // rbx
        let sp = sp.sub(1); *sp = 0; // r12
        let sp = sp.sub(1); *sp = 0; // r13
        let sp = sp.sub(1); *sp = 0; // r14
        let sp = sp.sub(1); *sp = 0; // r15
        thread.context.rsp = sp as u64;
    }

    unsafe {
        for i in 0..MAX_THREADS {
            if THREADS[i].is_none() {
                THREADS[i] = Some(thread);
                if let Some(ref mut p) = PROCESSES[0] {
                    p.thread_count += 1;
                }
                return Some(tid);
            }
        }
    }
    None
}

// ---- Sleep / Wakeup ----

/// Усыпить текущий поток на `ticks_to_sleep` тиков таймера.
/// ~18.2 тиков/сек при стандартном PIT (55 мс/тик).
pub fn sleep_ticks(ticks_to_sleep: u64) {
    unsafe {
        let cur = CURRENT_THREAD;
        if cur < 0 { return; }
        let idx = cur as usize;
        SLEEP_UNTIL[idx] = TICK_COUNT + ticks_to_sleep;
        if let Some(ref mut t) = THREADS[idx] {
            t.state = TaskState::Blocked;
        }
        schedule();
    }
}

/// Проверить и разбудить потоки, у которых истёк таймер сна.
fn wake_sleeping_threads() {
    unsafe {
        let now = TICK_COUNT;
        for i in 0..MAX_THREADS {
            if SLEEP_UNTIL[i] > 0 && now >= SLEEP_UNTIL[i] {
                SLEEP_UNTIL[i] = 0;
                if let Some(ref mut t) = THREADS[i] {
                    if t.state == TaskState::Blocked {
                        t.state = TaskState::Ready;
                    }
                }
            }
        }
    }
}

// ---- Timer ----

/// Тик таймера — вызывается из IRQ 0 обработчика.
///
/// Увеличивает счётчик тиков, будит спящие потоки, вызывает планировщик.
pub fn timer_tick() {
    unsafe { TICK_COUNT += 1; }
    wake_sleeping_threads();
    schedule();
}

/// Получить текущий счётчик тиков.
pub fn ticks() -> u64 {
    unsafe { TICK_COUNT }
}

/// Основная функция планировщика — выбрать и переключиться на следующий поток.
///
/// Алгоритм:
///   1. Текущий Running → Ready (если не Dead/Blocked)
///   2. Ищем следующий Ready поток (round-robin, приоритет учитывается)
///   3. Если нашли — переключаем контекст
///   4. Если не нашли — продолжаем idle
pub fn schedule() {
    unsafe {
        if !SCHEDULER_ACTIVE { return; }

        let current = CURRENT_THREAD;
        if current < 0 { return; }
        let cur = current as usize;

        // Текущий поток: Running → Ready (если жив)
        if let Some(ref mut t) = THREADS[cur] {
            if t.state == TaskState::Running {
                t.state = TaskState::Ready;
            }
        }

        // Ищем следующий Ready поток (round-robin)
        let mut next = None;
        let mut best_priority = Priority::Idle;

        // Первый проход: ищем с наивысшим приоритетом
        for i in 0..MAX_THREADS {
            let idx = (cur + 1 + i) % MAX_THREADS;
            if let Some(ref t) = THREADS[idx] {
                if t.state == TaskState::Ready && t.priority >= best_priority {
                    best_priority = t.priority;
                    next = Some(idx);
                }
            }
        }

        // Если ничего не нашли — остаёмся на текущем
        let next_idx = match next {
            Some(idx) => idx,
            None => {
                // Возвращаем текущий в Running
                if let Some(ref mut t) = THREADS[cur] {
                    if t.state == TaskState::Ready {
                        t.state = TaskState::Running;
                    }
                }
                return;
            }
        };

        // Если тот же поток — ничего не делаем
        if next_idx == cur {
            if let Some(ref mut t) = THREADS[cur] {
                t.state = TaskState::Running;
            }
            return;
        }

        // Переключаем
        THREADS[next_idx].as_mut().unwrap().state = TaskState::Running;
        CURRENT_THREAD = next_idx as i32;

        // Получаем указатели на RSP обоих потоков
        let old_rsp_ptr = &mut THREADS[cur].as_mut().unwrap().context.rsp as *mut u64;
        let new_rsp = THREADS[next_idx].as_ref().unwrap().context.rsp;

        // Контекст-свитч!
        switch_to(old_rsp_ptr, new_rsp);
    }
}

// ---- API для syscall и ipc ----

/// Завершить текущий поток.
pub fn exit_current() {
    unsafe {
        let cur = CURRENT_THREAD;
        if cur < 0 { return; }
        if let Some(ref mut t) = THREADS[cur as usize] {
            t.state = TaskState::Dead;
        }
        schedule(); // переключиться на другой поток
    }
}

/// Заблокировать текущий поток (для IPC, sleep и т.д.).
pub fn block_current() {
    unsafe {
        let cur = CURRENT_THREAD;
        if cur < 0 { return; }
        if let Some(ref mut t) = THREADS[cur as usize] {
            t.state = TaskState::Blocked;
        }
        schedule();
    }
}

/// Разблокировать поток по индексу.
pub fn unblock_thread(idx: usize) {
    unsafe {
        if idx < MAX_THREADS {
            if let Some(ref mut t) = THREADS[idx] {
                if t.state == TaskState::Blocked {
                    t.state = TaskState::Ready;
                }
            }
        }
    }
}

/// Получить PID текущего процесса.
pub fn current_pid() -> u64 {
    unsafe {
        let cur = CURRENT_THREAD;
        if cur < 0 { return 0; }
        match &THREADS[cur as usize] {
            Some(t) => t.pid,
            None => 0,
        }
    }
}

/// Получить TID текущего потока.
pub fn current_tid() -> u64 {
    unsafe {
        let cur = CURRENT_THREAD;
        if cur < 0 { return 0; }
        match &THREADS[cur as usize] {
            Some(t) => t.tid,
            None => 0,
        }
    }
}

/// Количество активных потоков.
pub fn thread_count() -> usize {
    unsafe {
        let mut count = 0;
        for i in 0..MAX_THREADS {
            if let Some(ref t) = THREADS[i] {
                if t.state != TaskState::Dead {
                    count += 1;
                }
            }
        }
        count
    }
}

/// Количество активных процессов.
pub fn process_count() -> usize {
    unsafe {
        let mut count = 0;
        for i in 0..MAX_PROCESSES {
            if PROCESSES[i].is_some() {
                count += 1;
            }
        }
        count
    }
}

// ---- Диагностика ----

/// Вывести список всех активных потоков (аналог `ps` в Unix).
pub fn list_threads() {
    crate::println!("  TID  PID  Pri   State     Name");
    crate::println!("  ---  ---  ---   -----     ----");
    unsafe {
        for i in 0..MAX_THREADS {
            if let Some(ref t) = THREADS[i] {
                if t.state == TaskState::Dead { continue; }
                let state_str = match t.state {
                    TaskState::Created => "created",
                    TaskState::Ready   => "ready  ",
                    TaskState::Running => "RUNNING",
                    TaskState::Blocked => "blocked",
                    TaskState::Dead    => "dead   ",
                };
                let pri_str = match t.priority {
                    Priority::Idle     => "idle",
                    Priority::Low      => "low ",
                    Priority::Normal   => "norm",
                    Priority::High     => "high",
                    Priority::Realtime => "rt  ",
                };
                // Декодируем имя
                let mut name_len = 0;
                while name_len < 32 && t.name[name_len] != 0 {
                    name_len += 1;
                }
                let name = unsafe {
                    core::str::from_utf8_unchecked(&t.name[..name_len])
                };
                let marker = if CURRENT_THREAD == i as i32 { ">" } else { " " };
                crate::println!("{} {:3}  {:3}  {}   {}   {}",
                    marker, t.tid, t.pid, pri_str, state_str, name);
            }
        }
    }
}

/// Получить имя потока по индексу в таблице.
pub fn get_thread_name(idx: usize) -> Option<&'static str> {
    unsafe {
        if idx >= MAX_THREADS { return None; }
        if let Some(ref t) = THREADS[idx] {
            let mut len = 0;
            while len < 32 && t.name[len] != 0 { len += 1; }
            Some(core::str::from_utf8_unchecked(&t.name[..len]))
        } else {
            None
        }
    }
}
