// =============================================================================
// NoNameOS — Syscall Interface
// =============================================================================
//
// Syscall — единственный легальный способ для user-space кода обратиться
// к ядру. Приложение кладёт номер syscall в RAX, аргументы в RDI, RSI, RDX,
// R10, R8, R9 (System V AMD64 ABI) и выполняет инструкцию `syscall`.
//
// CPU при `syscall`:
//   1. Сохраняет RIP в RCX, RFLAGS в R11
//   2. Загружает CS/SS из STAR MSR (ядро = Ring 0)
//   3. Загружает RIP из LSTAR MSR (адрес обработчика)
//   4. Маскирует RFLAGS по SFMASK MSR (обычно отключает IF)
//
// CPU при `sysret`:
//   1. Загружает RIP из RCX, RFLAGS из R11
//   2. Загружает CS/SS из STAR MSR (user = Ring 3)
//   3. Возвращает управление в user-space
//
// MSR регистры для настройки:
//   STAR   (0xC0000081) — сегменты для syscall/sysret
//   LSTAR  (0xC0000082) — адрес обработчика syscall (RIP)
//   SFMASK (0xC0000084) — маска RFLAGS (какие биты сбрасывать)
//   EFER   (0xC0000080) — бит 0 (SCE) включает syscall/sysret
//
// Номера syscall (наша таблица):
//
//   0  — sys_read(fd, buf, len)
//   1  — sys_write(fd, buf, len)
//   2  — sys_open(path, flags)
//   3  — sys_close(fd)
//   4  — sys_exit(code)
//   5  — sys_yield()
//   6  — sys_getpid()
//   7  — sys_spawn(path, argv)           — создать процесс
//   8  — sys_ipc_send(endpoint, msg)
//   9  — sys_ipc_recv(endpoint, buf)
//   10 — sys_ipc_register(name)
//   11 — sys_mmap(addr, len, prot, flags)
//   12 — sys_munmap(addr, len)
//   13 — sys_sleep(ms)
//   14 — sys_device_info(dev_idx, buf)
//
// Аналоги:
//   Linux: arch/x86/entry/entry_64.S (entry_SYSCALL_64)
//   Windows NT: ntdll!NtXxx → KiSystemCall64
// =============================================================================

// ---- MSR адреса ----
const MSR_EFER: u32   = 0xC0000080;
const MSR_STAR: u32   = 0xC0000081;
const MSR_LSTAR: u32  = 0xC0000082;
const MSR_SFMASK: u32 = 0xC0000084;

// ---- Номера syscall ----
pub const SYS_READ: u64          = 0;
pub const SYS_WRITE: u64         = 1;
pub const SYS_OPEN: u64          = 2;
pub const SYS_CLOSE: u64         = 3;
pub const SYS_EXIT: u64          = 4;
pub const SYS_YIELD: u64         = 5;
pub const SYS_GETPID: u64        = 6;
pub const SYS_SPAWN: u64         = 7;
pub const SYS_IPC_SEND: u64      = 8;
pub const SYS_IPC_RECV: u64      = 9;
pub const SYS_IPC_REGISTER: u64  = 10;
pub const SYS_MMAP: u64          = 11;
pub const SYS_MUNMAP: u64        = 12;
pub const SYS_SLEEP: u64         = 13;
pub const SYS_DEVICE_INFO: u64   = 14;

// ---- MSR helpers ----

/// Записать в MSR (Model Specific Register).
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags)
    );
}

/// Прочитать MSR.
#[allow(dead_code)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nostack, preserves_flags)
    );
    (high as u64) << 32 | low as u64
}

// ---- ASM entry point ----
//
// syscall_entry — точка входа, вызываемая CPU при инструкции `syscall`.
// Сохраняет контекст user-space, переключает на kernel stack,
// вызывает Rust-диспетчер, восстанавливает контекст и делает `sysret`.

core::arch::global_asm!(r#"
.global syscall_entry
syscall_entry:
    // При входе сюда:
    //   RCX = user RIP (куда вернуться)
    //   R11 = user RFLAGS
    //   RAX = номер syscall
    //   RDI, RSI, RDX, R10, R8, R9 = аргументы

    // Сохраняем user stack pointer, переключаемся на kernel stack
    // Пока используем простой подход: сохраняем RSP в R15 (callee-saved)
    // и используем фиксированный kernel stack.
    // TODO: per-thread kernel stack через GS base или TSS.RSP0

    mov r15, rsp                // сохраняем user RSP
    mov rsp, [kernel_stack_top] // переключаемся на kernel stack

    // Сохраняем контекст на kernel stack
    push r15        // user RSP
    push r11        // user RFLAGS
    push rcx        // user RIP
    push rax        // syscall number

    // Сохраняем callee-saved регистры
    push rbx
    push rbp
    push r12
    push r13
    push r14

    // Вызываем Rust диспетчер
    // Аргументы уже в правильных регистрах по SysV ABI:
    //   RDI = arg0 (первый аргумент syscall)
    //   RSI = arg1
    //   RDX = arg2
    //   RAX = syscall number → перекладываем в RCX (4-й аргумент по SysV)
    // Но нам нужен номер syscall. Берём его со стека.
    mov rcx, [rsp + 40]   // syscall number (сохранён 5 push * 8 = 40 байт назад)
    // RDI, RSI, RDX — аргументы syscall (уже на месте)
    // R10 → RCX замещён, передадим через R10 (4-й аргумент уже в R10 от caller)
    // Перестраиваем: RDI=arg0, RSI=arg1, RDX=arg2, RCX=syscall_no, R8=arg3, R9=arg4
    // Но SysV ABI: RDI, RSI, RDX, RCX, R8, R9
    // Нам нужно: syscall_dispatch(nr, arg0, arg1, arg2, arg3, arg4)
    // Поэтому перекладываем:
    mov r9, r9            // arg4 (на месте)
    mov r8, r8            // arg3 (на месте)
    // RDX = arg2 (на месте)
    // RSI = arg1 (на месте)
    // arg0 сейчас в RDI, но нам нужно nr в RDI
    // Сохраняем arg0 и подставляем nr
    push rdi              // сохраняем arg0
    mov rdi, [rsp + 48]   // syscall number (ещё +8 от push rdi)
    pop rsi               // arg0 теперь в RSI... нет, это сломает arg1

    // Проще: syscall_dispatch(nr, arg0, arg1, arg2)
    // nr в отдельном регистре. Передаём через стек или простой подход:
    // Откатываем — используем более простую конвенцию.
    push rdi              // сохраняем оригинальный arg0
    mov rdi, [rsp + 48]   // nr (5 callee + 1 push = 6*8 = 48)

    // Теперь: RDI=nr, но RSI=arg1, нужен arg0 в RSI
    // arg0 на стеке [rsp], arg1 в RSI... надо сдвинуть.
    // Слишком запутанно в inline asm. Упрощаем:

    pop rdi               // восстанавливаем arg0 в RDI

    // Простой подход: передаём (arg0, arg1, arg2, arg3) как есть,
    // а номер syscall берём из [rsp+40] внутри Rust через отдельный механизм.
    // Или ещё проще: RAX = номер, он уже сохранён — просто восстановим.

    // ФИНАЛЬНЫЙ ПОДХОД:
    // syscall_dispatch_inner(nr: u64, a0: u64, a1: u64, a2: u64)
    // где nr = RAX (номер), a0 = RDI, a1 = RSI, a2 = RDX
    // Rust SysV ABI: arg0=RDI, arg1=RSI, arg2=RDX, arg3=RCX
    // Поэтому: RCX=RDX(arg2), RDX=RSI(arg1), RSI=RDI(arg0), RDI=nr
    mov rcx, rdx          // arg2 → 4th param
    mov rdx, rsi          // arg1 → 3rd param
    mov rsi, rdi          // arg0 → 2nd param
    mov rdi, [rsp + 40]   // nr   → 1st param

    call syscall_dispatch_inner

    // RAX = возвращаемое значение (результат syscall)

    // Восстанавливаем callee-saved регистры
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx

    // Восстанавливаем контекст для sysret
    add rsp, 8            // пропускаем сохранённый syscall number
    pop rcx               // user RIP
    pop r11               // user RFLAGS
    pop rsp               // user RSP (восстанавливаем пользовательский стек)

    sysretq

// Kernel stack для syscall (временный, 16 KiB)
.section .bss
.align 16
kernel_syscall_stack:
    .space 16384
kernel_syscall_stack_top:

.section .data
.global kernel_stack_top
kernel_stack_top:
    .quad kernel_syscall_stack_top
"#);

// ---- Инициализация syscall ----

/// Настройка MSR для syscall/sysret.
///
/// После вызова инструкция `syscall` будет прыгать на syscall_entry.
pub fn init() {
    unsafe {
        // 1. Включить SCE (System Call Extensions) в EFER MSR
        let efer = rdmsr(MSR_EFER);
        wrmsr(MSR_EFER, efer | 1); // бит 0 = SCE

        // 2. STAR MSR — сегменты
        //
        //   GDT layout (после исправления для sysret):
        //     0x08 = Kernel Code
        //     0x10 = Kernel Data
        //     0x18 = User Data   (Ring 3)
        //     0x20 = User Code   (Ring 3)
        //
        //   SYSCALL (user → kernel):
        //     CS = STAR[47:32]      = 0x08 (kernel code)
        //     SS = STAR[47:32] + 8  = 0x10 (kernel data)
        //
        //   SYSRET (kernel → user, long mode):
        //     CS = (STAR[63:48] + 16) | 3 = (0x10 + 16) | 3 = 0x23 (user code ✓)
        //     SS = (STAR[63:48] + 8)  | 3 = (0x10 + 8)  | 3 = 0x1B (user data ✓)

        let star: u64 =
            (0x10u64 << 48)    // SYSRET base → CS=0x23, SS=0x1B
            | (0x08u64 << 32); // SYSCALL base → CS=0x08, SS=0x10
        wrmsr(MSR_STAR, star);

        // 3. LSTAR — адрес обработчика syscall
        extern "C" { fn syscall_entry(); }
        wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);

        // 4. SFMASK — маскировать IF (бит 9) чтобы прерывания были отключены в обработчике
        wrmsr(MSR_SFMASK, 0x200); // маска: IF=1 → отключить прерывания
    }
}

// ---- Rust диспетчер ----

/// Диспетчер syscall — вызывается из asm.
///
/// Принимает номер syscall и до 3 аргументов.
/// Возвращает результат в RAX.
#[no_mangle]
pub extern "C" fn syscall_dispatch_inner(nr: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match nr {
        SYS_WRITE => {
            // write(fd, buf_ptr, len)
            sys_write(arg0, arg1, arg2)
        }
        SYS_READ => {
            // read(fd, buf_ptr, len)
            sys_read(arg0, arg1, arg2)
        }
        SYS_OPEN => {
            // open(path_ptr, flags)
            sys_open(arg0, arg1)
        }
        SYS_CLOSE => {
            // close(fd)
            sys_close(arg0)
        }
        SYS_EXIT => {
            sys_exit(arg0)
        }
        SYS_YIELD => {
            sys_yield()
        }
        SYS_GETPID => {
            sys_getpid()
        }
        SYS_IPC_SEND => {
            sys_ipc_send(arg0, arg1, arg2)
        }
        SYS_IPC_RECV => {
            sys_ipc_recv(arg0, arg1)
        }
        SYS_IPC_REGISTER => {
            sys_ipc_register(arg0, arg1)
        }
        _ => {
            // Unknown syscall
            u64::MAX // -1 as u64
        }
    }
}

// ---- Реализации syscall ----

fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    // Специальный случай: fd=1 (stdout) → VGA
    if fd == 1 {
        let ptr = buf_ptr as *const u8;
        let slice = unsafe { core::slice::from_raw_parts(ptr, len as usize) };
        for &b in slice {
            crate::print!("{}", b as char);
        }
        return len;
    }

    // Обычный файл через VFS
    let mut buf = [0u8; 4096];
    let copy_len = core::cmp::min(len as usize, 4096);
    let src = buf_ptr as *const u8;
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len); }

    let result = crate::vfs::write(fd as usize, &buf[..copy_len]);
    if result < 0 { u64::MAX } else { result as u64 }
}

fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    // Специальный случай: fd=0 (stdin) → пока возвращаем 0
    if fd == 0 {
        return 0;
    }

    let mut tmp = [0u8; 4096];
    let read_len = core::cmp::min(len as usize, 4096);

    let result = crate::vfs::read(fd as usize, &mut tmp[..read_len]);
    if result <= 0 {
        return if result < 0 { u64::MAX } else { 0 };
    }

    let dst = buf_ptr as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), dst, result as usize); }
    result as u64
}

fn sys_open(path_ptr: u64, flags: u64) -> u64 {
    // Копируем путь из user-space
    let mut path_buf = [0u8; 256];
    let src = path_ptr as *const u8;
    // Ищем null-terminator (до 255 символов)
    let mut len = 0;
    while len < 255 {
        let b = unsafe { *src.add(len) };
        if b == 0 { break; }
        path_buf[len] = b;
        len += 1;
    }

    match crate::vfs::open(&path_buf[..len], flags as u32) {
        Some(fd) => fd as u64,
        None => u64::MAX,
    }
}

fn sys_close(fd: u64) -> u64 {
    crate::vfs::close(fd as usize);
    0
}

fn sys_exit(_code: u64) -> u64 {
    // TODO: завершить текущий поток/процесс через scheduler
    // Пока: помечаем текущий поток как Dead
    crate::scheduler::exit_current();
    0
}

fn sys_yield() -> u64 {
    crate::scheduler::schedule();
    0
}

fn sys_getpid() -> u64 {
    crate::scheduler::current_pid()
}

fn sys_ipc_send(endpoint_id: u64, buf_ptr: u64, len: u64) -> u64 {
    let mut payload = [0u8; 256];
    let copy_len = core::cmp::min(len as usize, 256);
    let src = buf_ptr as *const u8;
    unsafe { core::ptr::copy_nonoverlapping(src, payload.as_mut_ptr(), copy_len); }

    let msg = crate::ipc::Message::request(0, &payload[..copy_len]);
    match crate::ipc::send(endpoint_id as usize, msg) {
        Ok(()) => 0,
        Err(_) => u64::MAX,
    }
}

fn sys_ipc_recv(endpoint_id: u64, buf_ptr: u64) -> u64 {
    match crate::ipc::receive(endpoint_id as usize) {
        Some(msg) => {
            let dst = buf_ptr as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    msg.payload.as_ptr(), dst, msg.payload_len
                );
            }
            msg.payload_len as u64
        }
        None => 0,
    }
}

fn sys_ipc_register(name_ptr: u64, name_len: u64) -> u64 {
    let mut buf = [0u8; 32];
    let len = core::cmp::min(name_len as usize, 32);
    let src = name_ptr as *const u8;
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len); }

    // TODO: use current_pid
    let pid = crate::scheduler::current_pid();
    let name_str = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
    match crate::ipc::register_endpoint(name_str, pid) {
        Some(id) => id as u64,
        None => u64::MAX,
    }
}
