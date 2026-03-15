# =============================================================================
# NoNameOS — Build System
# =============================================================================
# Targets:
#   make              — build kernel.bin (debug)
#   make release      — build kernel.bin (release, LTO)
#   make iso          — create bootable ISO with GRUB2
#   make run          — build ISO and run in QEMU
#   make run-dbg      — run in QEMU with GDB server (port 1234)
#   make vdi          — create VDI disk image for VirtualBox
#   make vmdk         — create VMDK disk image for VMware
#   make clean        — remove all build artifacts
#   make help         — show this help
#
# Requirements:
#   nasm, cargo (nightly), ld/ld.lld, grub-mkrescue, xorriso, qemu
#
# =============================================================================

ASM       = nasm
CARGO     = cargo
LD       ?= ld

KERNEL_DIR    = kernel
BOOT_DIR      = bootloader
LINKER_DIR    = linker
BUILD_DIR     = build
ISO_DIR       = $(BUILD_DIR)/iso

KERNEL_BIN    = $(BUILD_DIR)/kernel.bin
ISO_FILE      = $(BUILD_DIR)/nonameos.iso
VDI_FILE      = $(BUILD_DIR)/nonameos.vdi
VMDK_FILE     = $(BUILD_DIR)/nonameos.vmdk

BOOT_ASM      = $(BOOT_DIR)/boot.asm
BOOT_OBJ      = $(BUILD_DIR)/boot.o
LINKER_SCRIPT = $(LINKER_DIR)/kernel.ld

RUST_TARGET   = x86_64-unknown-none

# Default: debug build
CARGO_FLAGS  ?=
RUST_LIB      = $(KERNEL_DIR)/target/$(RUST_TARGET)/debug/libnonameos_kernel.a

QEMU          = qemu-system-x86_64
QEMU_FLAGS    = -serial stdio -m 256M -no-reboot -no-shutdown -vga std

.PHONY: all release clean iso run run-dbg kernel kernel-release vdi vmdk help

all: $(KERNEL_BIN)
	@echo ""
	@echo "==> kernel.bin ready: $(KERNEL_BIN)"
	@echo "    Run: make run"
	@echo ""

# ---- Release build ----
release: CARGO_FLAGS = --release
release: RUST_LIB = $(KERNEL_DIR)/target/$(RUST_TARGET)/release/libnonameos_kernel.a
release: $(BOOT_OBJ) kernel-release
	$(LD) -n -T $(LINKER_SCRIPT) -o $(KERNEL_BIN) $(BOOT_OBJ) $(RUST_LIB)
	@echo "==> Release kernel built: $(KERNEL_BIN) ($$(stat -c%s $(KERNEL_BIN) 2>/dev/null || stat -f%z $(KERNEL_BIN)) bytes)"

# ---- Assemble boot.asm → boot.o ----
$(BOOT_OBJ): $(BOOT_ASM)
	@mkdir -p $(BUILD_DIR)
	$(ASM) -f elf64 $< -o $@

# ---- Build Rust kernel static library ----
kernel:
	cd $(KERNEL_DIR) && $(CARGO) build --target $(RUST_TARGET) $(CARGO_FLAGS)

kernel-release:
	cd $(KERNEL_DIR) && $(CARGO) build --target $(RUST_TARGET) --release

# ---- Link boot.o + Rust lib → kernel.bin ----
$(KERNEL_BIN): $(BOOT_OBJ) kernel
	$(LD) -n -T $(LINKER_SCRIPT) -o $@ $(BOOT_OBJ) $(RUST_LIB)

# ---- Create bootable ISO via GRUB2 ----
iso: $(KERNEL_BIN)
	@mkdir -p $(ISO_DIR)/boot/grub
	cp $(KERNEL_BIN) $(ISO_DIR)/boot/kernel.bin
	cp $(BOOT_DIR)/grub.cfg $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR) 2>/dev/null
	@echo ""
	@echo "==> ISO created: $(ISO_FILE)"
	@echo "    Boot in QEMU:      make run"
	@echo "    Boot in VirtualBox: make vdi"
	@echo "    Boot in VMware:     make vmdk"
	@echo ""

# ---- Run in QEMU ----
run: iso
	$(QEMU) -cdrom $(ISO_FILE) $(QEMU_FLAGS)

# ---- Run with GDB server for debugging ----
run-dbg: iso
	@echo "GDB server on localhost:1234. Connect with: target remote :1234"
	$(QEMU) -cdrom $(ISO_FILE) $(QEMU_FLAGS) -s -S

# ---- VM disk images ----
vdi: iso
	@rm -f $(VDI_FILE)
	qemu-img convert -f raw -O vdi $(ISO_FILE) $(VDI_FILE)
	@echo "==> VDI created: $(VDI_FILE)"
	@echo "    VirtualBox: New VM → Other/Unknown 64-bit → Use existing disk"

vmdk: iso
	@rm -f $(VMDK_FILE)
	qemu-img convert -f raw -O vmdk $(ISO_FILE) $(VMDK_FILE)
	@echo "==> VMDK created: $(VMDK_FILE)"
	@echo "    VMware: New VM → Use existing disk"

# ---- Clean everything ----
clean:
	rm -rf $(BUILD_DIR)
	cd $(KERNEL_DIR) && $(CARGO) clean

# ---- Help ----
help:
	@echo "NoNameOS Build System"
	@echo ""
	@echo "  make              Build kernel (debug)"
	@echo "  make release      Build kernel (release, LTO)"
	@echo "  make iso          Create bootable GRUB ISO"
	@echo "  make run          Build + run in QEMU"
	@echo "  make run-dbg      Run in QEMU with GDB (port 1234)"
	@echo "  make vdi          Create VDI for VirtualBox"
	@echo "  make vmdk         Create VMDK for VMware"
	@echo "  make clean        Remove build artifacts"
	@echo ""
	@echo "Requirements:"
	@echo "  nasm cargo ld grub-mkrescue xorriso mtools qemu"
	@echo "  Ubuntu: sudo apt install nasm grub-pc-bin xorriso mtools qemu-system-x86"
	@echo ""
