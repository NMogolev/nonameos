# =============================================================================
# NoNameOS — Build System (Windows PowerShell)
# =============================================================================
#
# Usage:
#   .\build.ps1                — build kernel.bin
#   .\build.ps1 -iso           — create bootable ISO (requires WSL + grub)
#   .\build.ps1 -run           — build ISO and run in QEMU
#   .\build.ps1 -vbox          — create VDI disk image for VirtualBox
#   .\build.ps1 -vmware        — create VMDK disk image for VMware
#   .\build.ps1 -clean         — remove build artifacts
#   .\build.ps1 -all           — kernel + ISO
#
# Requirements:
#   - Rust nightly with x86_64-unknown-none target
#   - NASM (nasm.us) — assembler
#   - x86_64-elf-ld or ld.lld — cross-linker
#   - WSL with grub-mkrescue + xorriso (for ISO)
#   - QEMU (for -run)
#   - qemu-img (for -vbox / -vmware)
#
# =============================================================================

param(
    [switch]$iso,
    [switch]$run,
    [switch]$vbox,
    [switch]$vmware,
    [switch]$clean,
    [switch]$all,
    [switch]$release
)

$ErrorActionPreference = "Stop"

# ---- Paths ----
$ROOT       = $PSScriptRoot
$KERNEL_DIR = "$ROOT\kernel"
$BOOT_DIR   = "$ROOT\bootloader"
$LINKER_DIR = "$ROOT\linker"
$BUILD_DIR  = "$ROOT\build"
$ISO_DIR    = "$BUILD_DIR\iso"

$BOOT_ASM      = "$BOOT_DIR\boot.asm"
$BOOT_OBJ      = "$BUILD_DIR\boot.o"
$KERNEL_BIN    = "$BUILD_DIR\kernel.bin"
$ISO_FILE      = "$BUILD_DIR\nonameos.iso"
$VDI_FILE      = "$BUILD_DIR\nonameos.vdi"
$VMDK_FILE     = "$BUILD_DIR\nonameos.vmdk"
$LINKER_SCRIPT = "$LINKER_DIR\kernel.ld"

$RUST_TARGET = "x86_64-unknown-none"

if ($release) {
    $CARGO_PROFILE = "release"
    $RUST_LIB = "$KERNEL_DIR\target\$RUST_TARGET\release\libnonameos_kernel.a"
} else {
    $CARGO_PROFILE = "dev"
    $RUST_LIB = "$KERNEL_DIR\target\$RUST_TARGET\debug\libnonameos_kernel.a"
}

# ---- Colors ----
function Write-Step($msg) { Write-Host "[BUILD] $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "[  OK ] $msg" -ForegroundColor Green }
function Write-Err($msg)  { Write-Host "[FAIL] $msg" -ForegroundColor Red }
function Write-Warn($msg) { Write-Host "[WARN] $msg" -ForegroundColor Yellow }

# ---- Tool detection ----
function Find-Tool($name, $alternatives) {
    $all = @($name) + $alternatives
    foreach ($t in $all) {
        $found = Get-Command $t -ErrorAction SilentlyContinue
        if ($found) { return $found.Source }
    }
    return $null
}

$NASM  = Find-Tool "nasm" @()
$LD    = Find-Tool "x86_64-elf-ld" @("ld.lld", "ld")
$QEMU  = Find-Tool "qemu-system-x86_64" @()
$QEMU_IMG = Find-Tool "qemu-img" @()

# ---- Clean ----
if ($clean) {
    Write-Step "Cleaning build artifacts..."
    if (Test-Path $BUILD_DIR) { Remove-Item -Recurse -Force $BUILD_DIR }
    Push-Location $KERNEL_DIR
    cargo clean 2>$null
    Pop-Location
    Write-Ok "Clean complete."
    exit 0
}

# ---- Ensure build directory ----
if (-not (Test-Path $BUILD_DIR)) { New-Item -ItemType Directory -Path $BUILD_DIR | Out-Null }

# ---- Step 1: Assemble boot.asm ----
Write-Step "Assembling boot.asm..."

if (-not $NASM) {
    Write-Err "NASM not found. Install from https://nasm.us"
    Write-Host "  Windows: choco install nasm  OR  scoop install nasm"
    exit 1
}

& $NASM -f elf64 $BOOT_ASM -o $BOOT_OBJ
if ($LASTEXITCODE -ne 0) { Write-Err "NASM assembly failed."; exit 1 }
Write-Ok "boot.o assembled."

# ---- Step 2: Build Rust kernel ----
Write-Step "Building Rust kernel ($CARGO_PROFILE)..."

Push-Location $KERNEL_DIR
if ($release) {
    cargo build --target $RUST_TARGET --release
} else {
    cargo build --target $RUST_TARGET
}
$cargoResult = $LASTEXITCODE
Pop-Location

if ($cargoResult -ne 0) { Write-Err "Cargo build failed."; exit 1 }
Write-Ok "Rust kernel library built."

# ---- Step 3: Link → kernel.bin ----
Write-Step "Linking kernel.bin..."

if (-not $LD) {
    Write-Err "Cross-linker not found. Need x86_64-elf-ld or ld.lld"
    Write-Host "  Windows: scoop install llvm  (provides ld.lld)"
    Write-Host "  Or install x86_64-elf-binutils"
    exit 1
}

& $LD -n -T $LINKER_SCRIPT -o $KERNEL_BIN $BOOT_OBJ $RUST_LIB
if ($LASTEXITCODE -ne 0) { Write-Err "Linking failed."; exit 1 }

$size = (Get-Item $KERNEL_BIN).Length
Write-Ok "kernel.bin linked ($([math]::Round($size/1024)) KiB)."

# ---- If only kernel requested, stop here ----
if (-not $iso -and -not $run -and -not $all -and -not $vbox -and -not $vmware) {
    Write-Host ""
    Write-Host "  kernel.bin ready: $KERNEL_BIN" -ForegroundColor White
    Write-Host "  Run with: .\build.ps1 -iso -run" -ForegroundColor DarkGray
    Write-Host ""
    exit 0
}

# ---- Step 4: Create bootable ISO ----
if ($iso -or $run -or $all -or $vbox -or $vmware) {
    Write-Step "Creating bootable ISO..."

    # Create ISO directory structure
    $grubDir = "$ISO_DIR\boot\grub"
    if (-not (Test-Path $grubDir)) { New-Item -ItemType Directory -Path $grubDir -Force | Out-Null }

    Copy-Item $KERNEL_BIN "$ISO_DIR\boot\kernel.bin" -Force
    Copy-Item "$BOOT_DIR\grub.cfg" "$ISO_DIR\boot\grub\grub.cfg" -Force

    # Try WSL grub-mkrescue first
    $isoCreated = $false

    # Method 1: WSL
    $wsl = Get-Command "wsl" -ErrorAction SilentlyContinue
    if ($wsl -and -not $isoCreated) {
        Write-Step "Using WSL for grub-mkrescue..."

        # Convert Windows paths to WSL paths
        $wslIsoDir = ($ISO_DIR -replace '\\', '/' -replace '^([A-Za-z]):', '/mnt/$1').ToLower()
        $wslIsoFile = ($ISO_FILE -replace '\\', '/' -replace '^([A-Za-z]):', '/mnt/$1').ToLower()

        wsl bash -c "which grub-mkrescue > /dev/null 2>&1"
        if ($LASTEXITCODE -eq 0) {
            wsl bash -c "grub-mkrescue -o '$wslIsoFile' '$wslIsoDir' 2>/dev/null"
            if ($LASTEXITCODE -eq 0) {
                $isoCreated = $true
                Write-Ok "ISO created via WSL."
            } else {
                Write-Warn "grub-mkrescue failed in WSL. Trying xorriso..."
            }
        } else {
            Write-Warn "grub-mkrescue not found in WSL."
            Write-Host "  Install: wsl sudo apt install grub-pc-bin grub-common xorriso mtools"
        }
    }

    # Method 2: xorriso directly (if installed on Windows)
    if (-not $isoCreated) {
        $xorriso = Find-Tool "xorriso" @()
        if ($xorriso) {
            Write-Step "Using xorriso directly..."
            # Create a minimal El Torito bootable ISO
            # This requires a GRUB image — fall through if not available
            Write-Warn "Direct xorriso ISO creation requires GRUB EFI/BIOS image."
        }
    }

    # Method 3: Fallback — raw QEMU boot (no ISO needed)
    if (-not $isoCreated) {
        Write-Warn "Could not create ISO. QEMU can still boot kernel.bin directly:"
        Write-Host "  qemu-system-x86_64 -kernel $KERNEL_BIN -serial stdio -m 256M" -ForegroundColor Yellow
        Write-Host ""
        Write-Host "  To enable ISO creation, install in WSL:" -ForegroundColor DarkGray
        Write-Host "    sudo apt install grub-pc-bin grub-common xorriso mtools" -ForegroundColor DarkGray
        Write-Host ""

        # For -run, fall back to direct kernel boot
        if ($run) {
            Write-Step "Falling back to QEMU direct kernel boot..."
            if (-not $QEMU) {
                Write-Err "QEMU not found. Install from https://www.qemu.org"
                exit 1
            }
            & $QEMU `
                -kernel $KERNEL_BIN `
                -serial stdio `
                -m 256M `
                -no-reboot `
                -no-shutdown
            exit 0
        }
        exit 0
    }

    $isoSize = (Get-Item $ISO_FILE).Length
    Write-Ok "nonameos.iso created ($([math]::Round($isoSize/1024/1024, 1)) MiB)."
}

# ---- Step 5: Create VM disk images ----
if ($vbox) {
    Write-Step "Creating VDI image for VirtualBox..."
    if (-not $QEMU_IMG) {
        Write-Err "qemu-img not found. Install QEMU tools."
        exit 1
    }
    if (Test-Path $VDI_FILE) { Remove-Item $VDI_FILE -Force }
    & $QEMU_IMG convert -f raw -O vdi $ISO_FILE $VDI_FILE
    if ($LASTEXITCODE -eq 0) {
        Write-Ok "VDI created: $VDI_FILE"
        Write-Host "  VirtualBox: New VM → Type: Other/Unknown 64-bit → Use existing disk → select nonameos.vdi"
    } else {
        Write-Err "VDI creation failed."
    }
}

if ($vmware) {
    Write-Step "Creating VMDK image for VMware..."
    if (-not $QEMU_IMG) {
        Write-Err "qemu-img not found. Install QEMU tools."
        exit 1
    }
    if (Test-Path $VMDK_FILE) { Remove-Item $VMDK_FILE -Force }
    & $QEMU_IMG convert -f raw -O vmdk $ISO_FILE $VMDK_FILE
    if ($LASTEXITCODE -eq 0) {
        Write-Ok "VMDK created: $VMDK_FILE"
        Write-Host "  VMware: New VM → Use existing disk → select nonameos.vmdk"
    } else {
        Write-Err "VMDK creation failed."
    }
}

# ---- Step 6: Run in QEMU ----
if ($run) {
    Write-Step "Starting QEMU..."

    if (-not $QEMU) {
        Write-Err "QEMU not found. Install from https://www.qemu.org"
        Write-Host "  Windows: choco install qemu  OR  scoop install qemu"
        exit 1
    }

    Write-Host ""
    Write-Host "  ╔══════════════════════════════════════════╗" -ForegroundColor Cyan
    Write-Host "  ║  NoNameOS running in QEMU                ║" -ForegroundColor Cyan
    Write-Host "  ║  Serial output: this console             ║" -ForegroundColor Cyan
    Write-Host "  ║  Exit: Ctrl+A, X                         ║" -ForegroundColor Cyan
    Write-Host "  ╚══════════════════════════════════════════╝" -ForegroundColor Cyan
    Write-Host ""

    if (Test-Path $ISO_FILE) {
        & $QEMU `
            -cdrom $ISO_FILE `
            -serial stdio `
            -m 256M `
            -no-reboot `
            -no-shutdown `
            -vga std
    } else {
        & $QEMU `
            -kernel $KERNEL_BIN `
            -serial stdio `
            -m 256M `
            -no-reboot `
            -no-shutdown `
            -vga std
    }
}
