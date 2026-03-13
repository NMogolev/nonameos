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
