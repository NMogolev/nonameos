// =============================================================================
// NoNameOS — PS/2 Keyboard Driver
// =============================================================================
//
// Как работает клавиатура в x86:
//
//   1. Пользователь нажимает клавишу
//   2. Контроллер клавиатуры (i8042) генерирует IRQ 1
//   3. CPU вызывает наш обработчик (INT 33 после ремаппинга PIC)
//   4. Обработчик читает скан-код из порта 0x60
//   5. Мы переводим скан-код в символ (ASCII)
//
// Скан-коды (Scan Code Set 1):
//   Каждая клавиша имеет уникальный номер.
//   При НАЖАТИИ приходит скан-код (например, 0x1E = 'A').
//   При ОТПУСКАНИИ приходит скан-код + 0x80 (например, 0x9E = отпустили 'A').
//
// Модификаторы:
//   Shift, Ctrl, Alt — отслеживаем по нажатию/отпусканию.
//   CapsLock — переключается при каждом нажатии.
//
// Порты i8042:
//   0x60 — Data Port (чтение: скан-код, запись: команда устройству)
//   0x64 — Status/Command (чтение: статус, запись: команда контроллеру)
//
// Буфер ввода:
//   Скан-коды приходят по прерыванию (асинхронно).
//   Мы складываем готовые символы в кольцевой буфер.
//   Потребитель (shell, приложение) читает из буфера когда готов.
// =============================================================================

use spin::Mutex;

// ---- Состояние клавиатуры ----

/// Модификаторы — какие специальные клавиши зажаты прямо сейчас.
struct KeyboardState {
    left_shift: bool,
    right_shift: bool,
    caps_lock: bool,
    ctrl: bool,
    alt: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        KeyboardState {
            left_shift: false,
            right_shift: false,
            caps_lock: false,
            ctrl: false,
            alt: false,
        }
    }

    /// Shift зажат?
    fn shift(&self) -> bool {
        self.left_shift || self.right_shift
    }

    /// Нужна заглавная буква? (Shift XOR CapsLock)
    fn uppercase(&self) -> bool {
        self.shift() ^ self.caps_lock
    }
}

static STATE: Mutex<KeyboardState> = Mutex::new(KeyboardState::new());

// ---- Кольцевой буфер ввода ----

const INPUT_BUF_SIZE: usize = 256;

struct InputBuffer {
    data: [u8; INPUT_BUF_SIZE],
    read_pos: usize,   // откуда читаем
    write_pos: usize,  // куда пишем
    count: usize,      // сколько символов в буфере
}

impl InputBuffer {
    const fn new() -> Self {
        InputBuffer {
            data: [0; INPUT_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
        }
    }

    /// Положить символ в буфер. Если буфер полон — символ теряется.
    fn push(&mut self, ch: u8) {
        if self.count < INPUT_BUF_SIZE {
            self.data[self.write_pos] = ch;
            self.write_pos = (self.write_pos + 1) % INPUT_BUF_SIZE;
            self.count += 1;
        }
    }

    /// Достать символ из буфера. None если пусто.
    fn pop(&mut self) -> Option<u8> {
        if self.count == 0 {
            None
        } else {
            let ch = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % INPUT_BUF_SIZE;
            self.count -= 1;
            Some(ch)
        }
    }
}

static INPUT: Mutex<InputBuffer> = Mutex::new(InputBuffer::new());

// ---- Таблица скан-кодов → ASCII (Set 1, US layout) ----
//
// Индекс массива = скан-код.
// Значение = ASCII символ (0 = нет символа для этого скан-кода).
//
// Только основные клавиши. Расширенные (мультимедиа) начинаются с 0xE0.

static SCANCODE_TO_ASCII: [u8; 128] = [
    // 0x00-0x0F
    0, 27,                              // 0x00=none, 0x01=Escape
    b'1', b'2', b'3', b'4',            // 0x02-0x05
    b'5', b'6', b'7', b'8',            // 0x06-0x09
    b'9', b'0', b'-', b'=',            // 0x0A-0x0D
    8,    b'\t',                        // 0x0E=Backspace, 0x0F=Tab

    // 0x10-0x1F
    b'q', b'w', b'e', b'r',            // 0x10-0x13
    b't', b'y', b'u', b'i',            // 0x14-0x17
    b'o', b'p', b'[', b']',            // 0x18-0x1B
    b'\n', 0,                           // 0x1C=Enter, 0x1D=Left Ctrl
    b'a', b's',                         // 0x1E-0x1F

    // 0x20-0x2F
    b'd', b'f', b'g', b'h',            // 0x20-0x23
    b'j', b'k', b'l', b';',            // 0x24-0x27
    b'\'', b'`', 0,                     // 0x28-0x2A (0x2A=Left Shift)
    b'\\',                              // 0x2B
    b'z', b'x', b'c', b'v',            // 0x2C-0x2F

    // 0x30-0x3F
    b'b', b'n', b'm', b',',            // 0x30-0x33
    b'.', b'/',  0,   b'*',            // 0x34-0x37 (0x36=Right Shift, 0x37=Keypad *)
    0,    b' ',  0,    0,              // 0x38=Alt, 0x39=Space, 0x3A=CapsLock, 0x3B=F1
    0, 0, 0, 0,                         // 0x3C-0x3F (F2-F5)

    // 0x40-0x4F
    0, 0, 0, 0, 0, 0, 0, 0,            // F6-F10, NumLock, ScrollLock
    0, 0, 0, 0, 0, 0, 0, 0,            // Keypad keys

    // 0x50-0x5F
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,

    // 0x60-0x6F
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,

    // 0x70-0x7F
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
];

/// Shift-версии символов (верхний регистр / спецсимволы).
static SCANCODE_TO_ASCII_SHIFT: [u8; 128] = [
    // 0x00-0x0F
    0, 27,
    b'!', b'@', b'#', b'$',
    b'%', b'^', b'&', b'*',
    b'(', b')', b'_', b'+',
    8, b'\t',

    // 0x10-0x1F
    b'Q', b'W', b'E', b'R',
    b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}',
    b'\n', 0,
    b'A', b'S',

    // 0x20-0x2F
    b'D', b'F', b'G', b'H',
    b'J', b'K', b'L', b':',
    b'"', b'~', 0,
    b'|',
    b'Z', b'X', b'C', b'V',

    // 0x30-0x3F
    b'B', b'N', b'M', b'<',
    b'>', b'?', 0, b'*',
    0, b' ', 0, 0,
    0, 0, 0, 0,

    // 0x40-0x7F (то же что и без Shift для спецклавиш)
    0, 0, 0, 0, 0, 0, 0, 0,  0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,  0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,  0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,  0, 0, 0, 0, 0, 0, 0, 0,
];

// ---- Публичный API ----

/// Обработать скан-код от IRQ 1. Вызывается из idt.rs → interrupt_dispatch.
pub fn handle_scancode(scancode: u8) {
    // Скан-коды >= 0x80 — это отпускание клавиши (release)
    let released = scancode & 0x80 != 0;
    let code = scancode & 0x7F; // убираем бит release

    let mut state = STATE.lock();

    match code {
        // Модификаторы — обновляем состояние
        0x2A => state.left_shift = !released,   // Left Shift
        0x36 => state.right_shift = !released,  // Right Shift
        0x1D => state.ctrl = !released,         // Left Ctrl
        0x38 => state.alt = !released,          // Left Alt
        0x3A if !released => {
            state.caps_lock = !state.caps_lock; // CapsLock — toggle при нажатии
        }

        // Обычные клавиши — только при НАЖАТИИ (не при отпускании)
        _ if !released => {
            let ch = if state.uppercase() {
                SCANCODE_TO_ASCII_SHIFT[code as usize]
            } else {
                SCANCODE_TO_ASCII[code as usize]
            };

            if ch != 0 {
                // Эхо на экран (чтобы сразу видеть что печатаем)
                if ch == b'\n' {
                    crate::println!();
                } else if ch == 8 {
                    // Backspace — стереть последний символ
                    crate::print!("\x08 \x08");
                } else {
                    crate::print!("{}", ch as char);
                }

                // Положить в буфер для чтения приложениями
                INPUT.lock().push(ch);
            }
        }

        _ => {} // Отпускание обычных клавиш — игнорируем
    }
}

/// Прочитать один символ из буфера ввода. None если буфер пуст.
pub fn read_char() -> Option<u8> {
    INPUT.lock().pop()
}

/// Есть ли символы в буфере?
pub fn has_input() -> bool {
    INPUT.lock().count > 0
}
