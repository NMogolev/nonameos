use core::fmt;
use spin::Mutex;

const VGA_BUFFER_ADDR: usize = 0xb8000;
const VGA_WIDTH: usize = 80;
const VGA_HEIGHT: usize = 25;

#[derive(Clone, Copy)]
#[repr(u8)]
#[allow(dead_code)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Clone, Copy)]
pub struct ColorCode(u8);

impl ColorCode {
    pub const fn new(fg: Color, bg: Color) -> Self {
        ColorCode((bg as u8) << 4 | (fg as u8))
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct VgaChar {
    ascii: u8,
    color: ColorCode,
}

pub struct Writer {
    col: usize,
    row: usize,
    color: ColorCode,
}

impl Writer {
    pub const fn new() -> Self {
        Writer {
            col: 0,
            row: 0,
            color: ColorCode::new(Color::LightGreen, Color::Black),
        }
    }

    #[inline]
    fn vga_ptr(&self, row: usize, col: usize) -> *mut VgaChar {
        (VGA_BUFFER_ADDR + (row * VGA_WIDTH + col) * 2) as *mut VgaChar
    }

    pub fn clear(&mut self) {
        let blank = VgaChar { ascii: b' ', color: self.color };
        for row in 0..VGA_HEIGHT {
            for col in 0..VGA_WIDTH {
                unsafe { self.vga_ptr(row, col).write_volatile(blank); }
            }
        }
        self.col = 0;
        self.row = 0;
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.col >= VGA_WIDTH {
                    self.new_line();
                }
                let ch = VgaChar { ascii: byte, color: self.color };
                unsafe { self.vga_ptr(self.row, self.col).write_volatile(ch); }
                self.col += 1;
            }
        }
    }

    fn new_line(&mut self) {
        if self.row >= VGA_HEIGHT - 1 {
            self.scroll();
        } else {
            self.row += 1;
        }
        self.col = 0;
    }

    fn scroll(&mut self) {
        for row in 1..VGA_HEIGHT {
            for col in 0..VGA_WIDTH {
                let ch = unsafe { self.vga_ptr(row, col).read_volatile() };
                unsafe { self.vga_ptr(row - 1, col).write_volatile(ch); }
            }
        }
        let blank = VgaChar { ascii: b' ', color: self.color };
        for col in 0..VGA_WIDTH {
            unsafe { self.vga_ptr(VGA_HEIGHT - 1, col).write_volatile(blank); }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                _ => self.write_byte(0xfe),
            }
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

pub static WRITER: Mutex<Writer> = Mutex::new(Writer::new());

pub fn clear_screen() {
    WRITER.lock().clear();
}
