#!/bin/bash
# =============================================================================
# NoNameOS — Build System (Linux / WSL / macOS)
# =============================================================================
#
# Usage:
#   ./build.sh              — build kernel.bin
#   ./build.sh iso          — create bootable ISO
#   ./build.sh run          — build ISO and run in QEMU
#   ./build.sh vbox         — create VDI for VirtualBox
#   ./build.sh vmware       — create VMDK for VMware
#   ./build.sh clean        — remove build artifacts
#   ./build.sh release      — release build
#   ./build.sh release iso  — release ISO
#
# =============================================================================

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
NC='\033[0m'

step()  { echo -e "${CYAN}[BUILD]${NC} $1"; }
ok()    { echo -e "${GREEN}[  OK ]${NC} $1"; }
err()   { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }

ROOT="$(cd "$(dirname "$0")" && pwd)"
KERNEL_DIR="$ROOT/kernel"
BOOT_DIR="$ROOT/bootloader"
LINKER_DIR="$ROOT/linker"
BUILD_DIR="$ROOT/build"
ISO_DIR="$BUILD_DIR/iso"

BOOT_ASM="$BOOT_DIR/boot.asm"
BOOT_OBJ="$BUILD_DIR/boot.o"
KERNEL_BIN="$BUILD_DIR/kernel.bin"
ISO_FILE="$BUILD_DIR/nonameos.iso"
LINKER_SCRIPT="$LINKER_DIR/kernel.ld"

RUST_TARGET="x86_64-unknown-none"
CARGO_PROFILE="dev"

# Parse args
DO_ISO=0
DO_RUN=0
DO_VBOX=0
DO_VMWARE=0

for arg in "$@"; do
    case "$arg" in
        clean)
            step "Cleaning..."
            rm -rf "$BUILD_DIR"
            cd "$KERNEL_DIR" && cargo clean 2>/dev/null || true
            ok "Clean complete."
            exit 0
            ;;
        release) CARGO_PROFILE="release" ;;
        iso)     DO_ISO=1 ;;
        run)     DO_ISO=1; DO_RUN=1 ;;
        vbox)    DO_ISO=1; DO_VBOX=1 ;;
        vmware)  DO_ISO=1; DO_VMWARE=1 ;;
    esac
done

if [ "$CARGO_PROFILE" = "release" ]; then
    RUST_LIB="$KERNEL_DIR/target/$RUST_TARGET/release/libnonameos_kernel.a"
else
    RUST_LIB="$KERNEL_DIR/target/$RUST_TARGET/debug/libnonameos_kernel.a"
fi

# ---- Check tools ----
command -v nasm  >/dev/null 2>&1 || err "NASM not found. Install: sudo apt install nasm"
command -v cargo >/dev/null 2>&1 || err "Cargo not found. Install Rust: https://rustup.rs"

# Find linker
LD=""
for l in x86_64-elf-ld ld.lld ld; do
    if command -v "$l" >/dev/null 2>&1; then LD="$l"; break; fi
done
[ -z "$LD" ] && err "Cross-linker not found. Install: sudo apt install binutils"

mkdir -p "$BUILD_DIR"

# ---- Step 1: Assemble ----
step "Assembling boot.asm..."
nasm -f elf64 "$BOOT_ASM" -o "$BOOT_OBJ"
ok "boot.o assembled."

# ---- Step 2: Build Rust kernel ----
step "Building Rust kernel ($CARGO_PROFILE)..."
cd "$KERNEL_DIR"
if [ "$CARGO_PROFILE" = "release" ]; then
    cargo build --target "$RUST_TARGET" --release
else
    cargo build --target "$RUST_TARGET"
fi
cd "$ROOT"
ok "Rust kernel built."

# ---- Step 3: Link ----
step "Linking kernel.bin..."
"$LD" -n -T "$LINKER_SCRIPT" -o "$KERNEL_BIN" "$BOOT_OBJ" "$RUST_LIB"
SIZE=$(stat -f%z "$KERNEL_BIN" 2>/dev/null || stat -c%s "$KERNEL_BIN" 2>/dev/null || echo "?")
ok "kernel.bin linked (${SIZE} bytes)."

# ---- Step 4: ISO ----
if [ "$DO_ISO" -eq 1 ]; then
    step "Creating bootable ISO..."

    command -v grub-mkrescue >/dev/null 2>&1 || err \
        "grub-mkrescue not found. Install: sudo apt install grub-pc-bin grub-common xorriso mtools"

    mkdir -p "$ISO_DIR/boot/grub"
    cp "$KERNEL_BIN" "$ISO_DIR/boot/kernel.bin"
    cp "$BOOT_DIR/grub.cfg" "$ISO_DIR/boot/grub/grub.cfg"

    grub-mkrescue -o "$ISO_FILE" "$ISO_DIR" 2>/dev/null
    ISO_SIZE=$(stat -f%z "$ISO_FILE" 2>/dev/null || stat -c%s "$ISO_FILE" 2>/dev/null || echo "?")
    ok "nonameos.iso created (${ISO_SIZE} bytes)."
fi

# ---- Step 5: VM disk images ----
if [ "$DO_VBOX" -eq 1 ]; then
    step "Creating VDI for VirtualBox..."
    command -v qemu-img >/dev/null 2>&1 || err "qemu-img not found."
    rm -f "$BUILD_DIR/nonameos.vdi"
    qemu-img convert -f raw -O vdi "$ISO_FILE" "$BUILD_DIR/nonameos.vdi"
    ok "VDI created: $BUILD_DIR/nonameos.vdi"
fi

if [ "$DO_VMWARE" -eq 1 ]; then
    step "Creating VMDK for VMware..."
    command -v qemu-img >/dev/null 2>&1 || err "qemu-img not found."
    rm -f "$BUILD_DIR/nonameos.vmdk"
    qemu-img convert -f raw -O vmdk "$ISO_FILE" "$BUILD_DIR/nonameos.vmdk"
    ok "VMDK created: $BUILD_DIR/nonameos.vmdk"
fi

# ---- Step 6: Run ----
if [ "$DO_RUN" -eq 1 ]; then
    command -v qemu-system-x86_64 >/dev/null 2>&1 || err "QEMU not found."
    step "Starting QEMU..."
    echo ""
    echo "  ╔══════════════════════════════════════════╗"
    echo "  ║  NoNameOS running in QEMU                ║"
    echo "  ║  Serial: this console | Exit: Ctrl+A, X  ║"
    echo "  ╚══════════════════════════════════════════╝"
    echo ""

    qemu-system-x86_64 \
        -cdrom "$ISO_FILE" \
        -serial stdio \
        -m 256M \
        -no-reboot \
        -no-shutdown \
        -vga std
fi

# ---- Done ----
if [ "$DO_ISO" -eq 0 ] && [ "$DO_RUN" -eq 0 ]; then
    echo ""
    echo "  kernel.bin ready: $KERNEL_BIN"
    echo "  Run with: ./build.sh run"
    echo ""
fi
