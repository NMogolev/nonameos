# =============================================================================
# NoNameOS — Build System
# =============================================================================
# Targets:
#   make          — build kernel.bin
#   make iso      — create bootable ISO
#   make run      — build ISO and run in QEMU
#   make clean    — remove all build artifacts
# =============================================================================

ASM       = nasm
CARGO     = cargo
LD        = ld

KERNEL_DIR    = kernel
BOOT_DIR      = bootloader
LINKER_DIR    = linker
BUILD_DIR     = build
ISO_DIR       = $(BUILD_DIR)/iso

KERNEL_BIN    = $(BUILD_DIR)/kernel.bin
ISO_FILE      = $(BUILD_DIR)/nonameos.iso

BOOT_ASM      = $(BOOT_DIR)/boot.asm
BOOT_OBJ      = $(BUILD_DIR)/boot.o
LINKER_SCRIPT = $(LINKER_DIR)/kernel.ld

RUST_TARGET   = x86_64-unknown-none
RUST_LIB      = $(KERNEL_DIR)/target/$(RUST_TARGET)/release/libnonameos_kernel.a

.PHONY: all clean iso run kernel

all: $(KERNEL_BIN)

# ---- Assemble boot.asm → boot.o ----
$(BOOT_OBJ): $(BOOT_ASM)
	@mkdir -p $(BUILD_DIR)
	$(ASM) -f elf64 $< -o $@

# ---- Build Rust kernel static library ----
kernel:
	cd $(KERNEL_DIR) && $(CARGO) build --release

# ---- Link boot.o + Rust lib → kernel.bin ----
$(KERNEL_BIN): $(BOOT_OBJ) kernel
	$(LD) -n -T $(LINKER_SCRIPT) -o $@ $(BOOT_OBJ) $(RUST_LIB)
	@echo ""
	@echo "==> Kernel built: $@"
	@echo ""

# ---- Create bootable ISO via GRUB2 ----
iso: $(KERNEL_BIN)
	@mkdir -p $(ISO_DIR)/boot/grub
	cp $(KERNEL_BIN) $(ISO_DIR)/boot/kernel.bin
	cp $(BOOT_DIR)/grub.cfg $(ISO_DIR)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO_FILE) $(ISO_DIR)
	@echo ""
	@echo "==> ISO created: $(ISO_FILE)"
	@echo ""

# ---- Run in QEMU ----
run: iso
	qemu-system-x86_64 \
		-cdrom $(ISO_FILE) \
		-serial stdio \
		-m 256M \
		-no-reboot \
		-no-shutdown

# ---- Clean everything ----
clean:
	rm -rf $(BUILD_DIR)
	cd $(KERNEL_DIR) && $(CARGO) clean
