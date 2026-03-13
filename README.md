## Архитектура

```
┌─────────────────────────────────┐
│  User Applications (.exe)       │
├─────────────────────────────────┤
│  Compatibility Layer (WinAPI)   │
│  NTDLL · KERNEL32 · USER32      │
├─────────────────────────────────┤
│  System Services (user-space)   │
│  Registry · SCM · Plug&Play     │
├─────────────────────────────────┤
│  Drivers (user-space)           │
├─────────────────────────────────┤
│  Microkernel (Rust, no_std)     │
│  VM · Threads · IPC · IRQ       │
└─────────────────────────────────┘
```

## Технологии

| Компонент       | Язык / Технология      |
|-----------------|------------------------|
| Микроядро       | Rust (`no_std`)        |
| Загрузчик       | GRUB2 (Multiboot2)     |
| Bootstrap       | NASM (x86_64 asm)      |
| PE Loader       | Rust (планируется)     |
| Совместимость   | Rust / C               |
| Драйверы        | Rust / C + shim        |
| Графика         | Vulkan / Mesa          |

## Сборка

### Зависимости

```bash
# Rust nightly
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly

## Структура проекта

```
nonameos/
├── kernel/              # Rust-ядро (no_std, staticlib)
│   ├── src/
│   │   ├── lib.rs       # Точка входа kernel_main()
│   │   ├── vga.rs       # VGA текстовый вывод
│   │   ├── gdt.rs       # Global Descriptor Table
│   │   ├── idt.rs       # Interrupt Descriptor Table
│   │   └── serial.rs    # COM1 для отладки в QEMU
│   ├── Cargo.toml
│   └── .cargo/config.toml
├── bootloader/
│   ├── boot.asm         # Multiboot2 bootstrap → long mode
│   └── grub.cfg         # GRUB2 конфигурация
├── linker/
│   └── kernel.ld        # Линкер-скрипт для x86_64
├── Makefile             # Сборочная система
└── NoNameOS             # Манифест проекта
```
