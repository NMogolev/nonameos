# NoNameOS

Микроядерная операционная система на Rust для x86_64

## Архитектура

```
┌──────────────────────────────────────────┐
│  User Applications (.exe)                │
├──────────────────────────────────────────┤
│  Compatibility Layer (WinAPI)            │
│  NTDLL · KERNEL32 · USER32               │
├──────────────────────────────────────────┤
│  System Services (user-space)            │
│  Registry · SCM · Plug&Play              │
├──────────────────────────────────────────┤
│  Drivers (user-space, Linux shim)        │
├──────────────────────────────────────────┤
│  Microkernel (Rust, no_std, x86_64)      │
│                                          │
│  ┌────────┐ ┌────────┐ ┌─────────────┐   │
│  │Scheduler│ │Syscall│ │  Diagnostics│   │
│  │ RR+prio │ │ MSR  │ │  & Recovery  │   │
│  └───┬────┘ └───┬────┘ └──────┬──────┘   │
│      │          │              │         │
│  ┌───▼──┐ ┌────▼───┐ ┌───────▼──────┐    │
│  │ Task │ │  VFS   │ │    IPC       │    │
│  │Proc/ │ │ ramfs  │ │  endpoints   │    │
│  │Thread│ │ path   │ │  msg queue   │    │
│  └──────┘ └────────┘ └──────────────┘    │
│                                          │
│  ┌────────────────────────────────────┐  │
│  │  HAL: GDT · IDT · PIC · Memory     │  │
│  │  PCI · Serial · VGA · KBD          │  │
│  └────────────────────────────────────┘  │
└──────────────────────────────────────────┘
```

## Подсистемы ядра

| Модуль       | Описание                                                    |
|--------------|-------------------------------------------------------------|
| `gdt`        | GDT + TSS — сегменты kernel/user, Ring 0↔3 stack switching  |
| `idt`        | Interrupt Descriptor Table — обработчики прерываний и CPU исключений |
| `pic`        | PIC 8259 — маршрутизация аппаратных IRQ                     |
| `memory`     | Bitmap физический аллокатор (512 MiB), paging               |
| `vga`        | VGA text mode 80×25, макросы `print!`/`println!`            |
| `serial`     | COM1 порт — отладочный вывод через QEMU                     |
| `keyboard`   | PS/2 клавиатура с обработкой скан-кодов                     |
| `task`       | Структуры `Process` / `Thread`, контекст CPU, приоритеты    |
| `scheduler`  | Round-robin планировщик с приоритетами, context switch (asm) |
| `syscall`    | Интерфейс системных вызовов через MSR (STAR/LSTAR/SFMASK)   |
| `vfs`        | Виртуальная файловая система с ramfs, path lookup, POSIX-like API |
| `ipc`        | Message passing через именованные endpoints                 |
| `drivers`    | Device/Driver модель, PCI bus scan, Linux kernel API shim   |
| `ktest`      | Boot-time диагностика и recovery с health tracking          |
| `userspace`  | Создание user-mode процессов, iretq переход в Ring 3        |
| `loader`     | PE загрузчик — парсинг PE/COFF, маппинг секций, imports     |
| `win32`      | Слой совместимости Windows: типы, PE формат, NT API каркас  |

## Многопоточность

Ядро использует preemptive multitasking на основе таймера PIT (IRQ 0, ~18.2 Hz).

### Планировщик

- **Алгоритм**: Round-Robin с приоритетами (Idle → Low → Normal → High → Realtime)
- **Context switch**: ассемблерный `switch_to` — сохранение/восстановление callee-saved регистров + swap RSP
- **Preemptive**: по тику таймера планировщик переключает потоки автоматически
- **Cooperative**: `sleep_ticks()`, `block_current()`, `exit_current()`

### Kernel Worker Threads

При загрузке ядро спавнит рабочие потоки:

| Поток          | Приоритет | Интервал | Задача                                      |
|----------------|-----------|----------|----------------------------------------------|
| `idle`         | Idle      | —        | HLT loop, выполняется когда нечего делать    |
| `health_mon`   | High      | ~5 сек   | Проверка здоровья подсистем, алерты          |
| `stats`        | Low       | ~10 сек  | Сбор статистики (uptime, RAM, threads) в VFS |
| `reaper`       | Low       | ~2 сек   | Очистка Dead-потоков, освобождение ресурсов  |

### API

```rust
scheduler::spawn_kernel_thread(entry, "name")          // Normal priority
scheduler::spawn_kernel_thread_with_priority(entry, "name", Priority::High)
scheduler::sleep_ticks(91)                              // ~5 сек при PIT
scheduler::block_current() / unblock_thread(idx)        // IPC blocking
scheduler::list_threads()                               // ps-like диагностика
```

## Boot-Time Diagnostics & Recovery

При загрузке ядро прогоняет 19 диагностических тестов по 6 подсистемам.

### Уровни критичности

| Severity   | При фейле                                              |
|------------|--------------------------------------------------------|
| `Critical` | Recovery → если не помогло → kernel panic / minimal mode |
| `Required` | Recovery → если не помогло → degraded                   |
| `Optional` | Логируем, продолжаем                                    |

### Режимы загрузки

| Mode      | Условие                         | Поведение                          |
|-----------|----------------------------------|------------------------------------|
| `Normal`  | Все тесты ОК                    | Полная функциональность            |
| `Safe`    | ≥2 подсистемы деградированы     | Аналог Windows Safe Mode           |
| `Minimal` | Critical failure (scheduler/VFS) | Только memory + VGA + keyboard     |

### Recovery стратегии

- **Reinit** — повторная инициализация подсистемы (memory, VFS, scheduler)
- **Fallback** — переключение на упрощённый режим
- **Isolate** — отключение сломанного компонента
- **Panic** — полная остановка (только при отказе памяти)

### Runtime Health Monitoring

Поток `health_mon` каждые ~5 секунд вызывает `runtime_health_check()`,
отслеживая деградацию подсистем в реальном времени.

## Последовательность загрузки

```
boot.asm (Multiboot2) → kernel_main():
  1.  VGA text mode        — экран 80×25
  2.  Serial COM1          — отладочный порт
  3.  GDT                  — сегменты kernel + user
  4.  IDT + PIC            — прерывания (256 векторов)
  5.  Memory               — bitmap аллокатор физических фреймов
  6.  VFS                  — ramfs root (/, /dev, /tmp, /sys, /proc)
  7.  PCI Bus Scan         — обнаружение устройств, регистрация в device manager
  8.  Scheduler            — idle процесс (PID 0) и idle поток (TID 0)
  9.  Syscall              — настройка MSR для syscall/sysret
 10.  Worker Threads       — health_mon, stats, reaper
 11.  Interrupts ON        — multithreading активен
 12.  Boot Diagnostics     — 19 тестов, recovery при фейлах
 13.  Thread List          — вывод ps-like таблицы потоков
 14.  Ready                — idle loop (hlt)
 15.  User-Space Demo      — загрузка demo.exe → Ring 3 через iretq
```

## User-Space & PE Loader

### Переход в Ring 3

Ядро поддерживает полноценный переход из kernel mode (Ring 0) в user mode (Ring 3):

```
GDT Layout (критичен для sysret):
  0x08  Kernel Code  (Ring 0, 64-bit)
  0x10  Kernel Data  (Ring 0)
  0x18  User Data    (Ring 3)  ← sysret SS
  0x20  User Code    (Ring 3)  ← sysret CS
  0x28  TSS          (16 bytes, 2 слота)

TSS.RSP0 → стек ядра при прерывании из Ring 3
iretq    → первый вход в user-space
syscall  → user→kernel, sysret → kernel→user
```

### PE Loader

Полноценный загрузчик PE64 (.exe) файлов Windows:

1. **Парсинг**: DOS Header → PE Signature → COFF Header → Optional Header → Section Headers
2. **Адресное пространство**: новый PML4 с копией kernel mappings
3. **Маппинг заголовков**: первые SizeOfHeaders байт по ImageBase
4. **Маппинг секций**: .text, .data, .rdata, .bss → виртуальные адреса
5. **Import Table**: логирование зависимых DLL (заглушка для будущего)
6. **User Stack**: 64 KiB стек ниже 0x7FFF_FFFF_0000
7. **Entry Point**: ImageBase + AddressOfEntryPoint → RIP

### Адресное пространство процесса

```
0x0000_0000_0040_0000   ImageBase (PE default)
    ...                 PE секции (.text, .data, .rdata)
0x0000_7FFF_FFFE_0000   User stack bottom
0x0000_7FFF_FFFF_0000   User stack top
0xFFFF_8000_0000_0000+  Kernel space (shared)
```

## Syscall Interface

15 системных вызовов через `syscall` инструкцию x86_64:

| #  | Имя             | Описание                          |
|----|-----------------|-----------------------------------|
| 0  | `sys_read`      | Чтение из файлового дескриптора   |
| 1  | `sys_write`     | Запись в файловый дескриптор      |
| 2  | `sys_open`      | Открытие файла по пути            |
| 3  | `sys_close`     | Закрытие файлового дескриптора    |
| 4  | `sys_exit`      | Завершение текущего потока        |
| 5  | `sys_yield`     | Добровольная отдача CPU           |
| 6  | `sys_getpid`    | Получение PID текущего процесса   |
| 7  | `sys_spawn`     | Создание нового потока            |
| 8  | `sys_ipc_send`  | Отправка IPC сообщения            |
| 9  | `sys_ipc_recv`  | Приём IPC сообщения               |
| 10 | `sys_ipc_reg`   | Регистрация IPC endpoint          |

## Драйверная модель

```
Device Manager ←── register_device(dev)
     │
     ├── PCI Bus Driver (scan, BAR, bus mastering)
     ├── Platform Bus (virtual devices)
     └── Driver Matching (vendor:device wildcard)

Linux Shim Layer:
     kmalloc/kfree, ioremap, spinlock, atomic ops,
     in/out ports, printk — для портирования Linux драйверов
```

## Технологии

| Компонент       | Язык / Технология      |
|-----------------|------------------------|
| Микроядро       | Rust (`no_std`)        |
| Загрузчик       | GRUB2 (Multiboot2)     |
| Bootstrap       | NASM (x86_64 asm)      |
| Context switch  | x86_64 inline asm      |
| PE Loader       | Rust (реализован)      |
| Совместимость   | WinAPI shim (Rust/C)   |
| Драйверы        | Rust / C + Linux shim  |
| Графика         | Vulkan / Mesa (план.)  |

## Структура проекта

```
nonameos/
├── boot/
│   ├── boot.asm          # Multiboot2 bootstrap, GDT64, long mode
│   └── linker.ld         # Линкер-скрипт ядра
├── kernel/
│   ├── Cargo.toml        # no_std, x86_64-unknown-none
│   └── src/
│       ├── lib.rs         # Точка входа, boot sequence, worker threads
│       ├── gdt.rs         # Global Descriptor Table
│       ├── idt.rs         # Interrupt Descriptor Table, ISR dispatch
│       ├── pic.rs         # PIC 8259A
│       ├── memory/
│       │   ├── mod.rs     # Memory API
│       │   ├── phys.rs    # Bitmap frame allocator
│       │   └── paging.rs  # Page tables (identity map)
│       ├── vga.rs         # VGA text mode, spinlock writer
│       ├── serial.rs      # COM1 serial port
│       ├── keyboard.rs    # PS/2 keyboard driver
│       ├── task.rs        # Process/Thread structs, CpuContext
│       ├── scheduler.rs   # Round-robin scheduler, context switch asm
│       ├── syscall.rs     # syscall/sysret MSR setup, dispatch table
│       ├── vfs/
│       │   ├── mod.rs     # VFS core: inode, dentry, file ops
│       │   └── ramfs.rs   # RAM filesystem
│       ├── ipc.rs         # Message passing, named endpoints
│       ├── drivers/
│       │   ├── mod.rs     # Device/Driver model, registration
│       │   ├── bus/
│       │   │   ├── mod.rs
│       │   │   └── pci.rs # PCI config space, BAR, device scan
│       │   └── linux_shim/
│       │       └── mod.rs # Linux kernel API compatibility layer
│       ├── ktest.rs       # Boot diagnostics, recovery, health monitor
│       ├── userspace.rs   # User-mode процессы, iretq, demo binary
│       ├── loader.rs      # PE загрузчик (parse, map sections, imports)
│       └── win32/         # WinAPI compatibility layer
│           ├── mod.rs     # Архитектура совместимости
│           ├── types.rs   # Фундаментальные Win32/NT типы
│           ├── error.rs   # NTSTATUS коды ошибок
│           ├── object.rs  # Объектная модель (HANDLE, attributes)
│           ├── pe.rs      # PE/COFF структуры и валидация
│           ├── ntapi.rs   # NT API каркас (NtCreateFile и др.)
│           └── subsys.rs  # Win32k подсистема (окна, GDI)
└── Makefile               # Build: kernel.bin → nonameos.iso → QEMU
```

## Сборка

### Зависимости

```bash
# Rust nightly
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
