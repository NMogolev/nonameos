// =============================================================================
// NoNameOS — Win32 Subsystem Framework
// =============================================================================
//
//
// В Windows:
//
//   ┌─────────────────────────────────────────────────────────────┐
//   │                     USER-SPACE                              │
//   │                                                             │
//   │  USER32.DLL          │  GDI32.DLL         │  KERNEL32.DLL   │
//   │  - CreateWindow      │  - CreateDC         │  - CreateFile  │
//   │  - SendMessage       │  - BitBlt           │  - ReadFile    │
//   │  - GetMessage        │  - TextOut          │  - VirtualAlloc│
//   │  - DispatchMessage   │  - SelectObject     │  - CreateProc  │
//   └─────────┬────────────┴─────────┬───────────┴───────┬─────── ┘
//             │ syscall              │ syscall            │ syscall
//   ┌─────────▼──────────────────────▼───────────────────▼───────┐
//   │                     KERNEL-SPACE                           │
//   │                                                            │
//   │  WIN32K.SYS (win32ss в ReactOS):                           │
//   │  ┌──────────────────────────────────────────────────┐      │
//   │  │ Window Manager (USER)                            │      │
//   │  │  - Окна, сообщения, input, фокус, z-order        │      │
//   │  │  - Hit testing, клиппинг                         │      │
//   │  │  - Desktop, Window Station                       │      │
//   │  ├──────────────────────────────────────────────────┤      │
//   │  │ Graphics Engine (GDI)                            │      │
//   │  │  - DC (Device Context), кисти, шрифты            │      │
//   │  │  - Растеризация, BitBlt, TextOut                 │      │
//   │  │  - Драйверы дисплея                              │      │
//   │  └──────────────────────────────────────────────────┘      │
//   │                                                            │
//   │  NTOSKRNL.EXE:                                             │
//   │  - Object Manager, Memory Manager, I/O Manager, ...        │
//   └────────────────────────────────────────────────────────────┘
//
// В ядре:
//
//   Win32 подсистема работает В USER-SPACE как сервер!
//   Приложения общаются с ней через IPC.
//   Это безопаснее
//   и модульнее.
//
//   ┌──────────────────────────────────────────────────────────┐
//   │                     USER-SPACE                           │
//   │                                                          │
//   │  ┌──────────┐    IPC    ┌─────────────────────────┐      │
//   │  │ app.exe  │ ────────→ │ Win32 Server            │      │
//   │  │ (Win32)  │ ←──────── │  - Window Manager       │      │
//   │  └──────────┘           │  - GDI Engine           │      │
//   │                         │  - Console              │      │
//   │                         │  - Clipboard            │      │
//   │                         └─────────────────────────┘      │
//   │                                                          │
//   │  ┌──────────┐    IPC    ┌─────────────────────────┐      │
//   │  │ app.exe  │ ────────→ │ Registry Server         │      │
//   │  └──────────┘           └─────────────────────────┘      │
//   └──────────────────────────────────────────────────────────┘
//   ┌──────────────────────────────────────────────────────────┐
//   │                     KERNEL                               │
//   │  NoNameOS Microkernel (IPC, VM, Threads, IRQ)            │
//   └──────────────────────────────────────────────────────────┘
//
// Этот файл определяет КАРКАС подсистемы:
//   - Типы оконных сообщений
//   - Структуры окон
//   - GDI примитивы
//   - Консольный интерфейс
//
// Всё это — определения и интерфейсы. Реализация — в user-space сервере.
// Ядро знает об этих структурах, чтобы корректно маршрутизировать IPC.
//
// Источники:
//   - ReactOS: win32ss/user/ntuser/ (окна), win32ss/gdi/ (графика)
//   - Wine: dlls/user32/ (окна), dlls/gdi32/ (графика)
//   - MSDN: Window Messages, GDI Objects
// =============================================================================

use super::types::*;

// =============================================================================
// ОКОННАЯ ПОДСИСТЕМА (Window Manager / USER)
// =============================================================================
//
// Центральные концепции:
//
//   HWND — дескриптор окна (аналог HANDLE, но для окон)
//   MSG  — оконное сообщение (WM_PAINT, WM_KEYDOWN, WM_CLOSE...)
//   WNDCLASS — класс окна (шаблон: имя класса, процедура, стили)
//   WndProc — оконная процедура (callback для обработки сообщений)
//
// Цикл обработки сообщений (Message Loop):
//
//   while (GetMessage(&msg, NULL, 0, 0)) {
//       TranslateMessage(&msg);   // Клавиши → WM_CHAR
//       DispatchMessage(&msg);    // Вызвать WndProc окна
//   }

/// Дескриптор окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HWND(pub u64);

impl HWND {
    pub const NULL: HWND = HWND(0);

    pub fn is_null(self) -> bool { self.0 == 0 }
}

/// Специальные значения HWND.
pub const HWND_DESKTOP: HWND   = HWND(0);
pub const HWND_BROADCAST: HWND = HWND(0xFFFF);
pub const HWND_TOP: HWND       = HWND(0);
pub const HWND_BOTTOM: HWND    = HWND(1);
pub const HWND_TOPMOST: HWND   = HWND(u64::MAX - 1); // (HWND)-1
pub const HWND_NOTOPMOST: HWND = HWND(u64::MAX - 2); // (HWND)-2

/// Оконное сообщение.
///
/// Каждое взаимодействие с окном — это сообщение:
///   - Пользователь нажал клавишу → WM_KEYDOWN
///   - Нужно перерисовать окно → WM_PAINT
///   - Пользователь закрывает окно → WM_CLOSE
///   - Размер окна изменился → WM_SIZE
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MSG {
    pub hwnd: HWND,       // Окно-получатель
    pub message: DWORD,   // Код сообщения (WM_*)
    pub wparam: WPARAM,   // Параметр 1 (зависит от сообщения)
    pub lparam: LPARAM,   // Параметр 2 (зависит от сообщения)
    pub time: DWORD,      // Время отправки (GetTickCount)
    pub pt: POINT,        // Позиция курсора
}

/// Координаты точки.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct POINT {
    pub x: LONG,
    pub y: LONG,
}

/// Прямоугольник (координаты окна, клиентской области, и т.д.).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RECT {
    pub left: LONG,
    pub top: LONG,
    pub right: LONG,
    pub bottom: LONG,
}

impl RECT {
    pub fn width(&self) -> LONG { self.right - self.left }
    pub fn height(&self) -> LONG { self.bottom - self.top }
    pub fn is_empty(&self) -> bool { self.right <= self.left || self.bottom <= self.top }
}

// ---- Коды оконных сообщений (WM_*) ----
//
// Это лишь МАЛАЯ часть.
// Тут пока база.

pub const WM_NULL: DWORD            = 0x0000;
pub const WM_CREATE: DWORD          = 0x0001;  // Окно создано
pub const WM_DESTROY: DWORD         = 0x0002;  // Окно уничтожается
pub const WM_MOVE: DWORD            = 0x0003;  // Окно переместилось
pub const WM_SIZE: DWORD            = 0x0005;  // Размер изменился
pub const WM_ACTIVATE: DWORD        = 0x0006;  // Окно активировано/деактивировано
pub const WM_SETFOCUS: DWORD        = 0x0007;  // Получен фокус ввода
pub const WM_KILLFOCUS: DWORD       = 0x0008;  // Потерян фокус
pub const WM_ENABLE: DWORD          = 0x000A;  // Окно enabled/disabled
pub const WM_PAINT: DWORD           = 0x000F;  // Нужна перерисовка
pub const WM_CLOSE: DWORD           = 0x0010;  // Запрос на закрытие
pub const WM_QUIT: DWORD            = 0x0012;  // Выход из message loop
pub const WM_ERASEBKGND: DWORD      = 0x0014;  // Стереть фон
pub const WM_SHOWWINDOW: DWORD      = 0x0018;  // Показать/скрыть окно
pub const WM_SETTEXT: DWORD         = 0x000C;  // Установить текст заголовка
pub const WM_GETTEXT: DWORD         = 0x000D;  // Получить текст заголовка
pub const WM_GETTEXTLENGTH: DWORD   = 0x000E;  // Длина текста

// Input messages
pub const WM_KEYDOWN: DWORD         = 0x0100;  // Клавиша нажата
pub const WM_KEYUP: DWORD           = 0x0101;  // Клавиша отпущена
pub const WM_CHAR: DWORD            = 0x0102;  // Символ (после TranslateMessage)
pub const WM_SYSKEYDOWN: DWORD      = 0x0104;  // Alt+клавиша нажата
pub const WM_SYSKEYUP: DWORD        = 0x0105;  // Alt+клавиша отпущена
pub const WM_SYSCOMMAND: DWORD      = 0x0112;  // Системная команда (меню, close...)

// Mouse messages
pub const WM_MOUSEMOVE: DWORD       = 0x0200;
pub const WM_LBUTTONDOWN: DWORD     = 0x0201;
pub const WM_LBUTTONUP: DWORD       = 0x0202;
pub const WM_LBUTTONDBLCLK: DWORD   = 0x0203;
pub const WM_RBUTTONDOWN: DWORD     = 0x0204;
pub const WM_RBUTTONUP: DWORD       = 0x0205;
pub const WM_MOUSEWHEEL: DWORD      = 0x020A;

// Timer
pub const WM_TIMER: DWORD           = 0x0113;

// Command
pub const WM_COMMAND: DWORD         = 0x0111;  // Меню/кнопка/акселератор
pub const WM_NOTIFY: DWORD          = 0x004E;  // Уведомление от контрола

// ---- Стили окон (Window Styles) ----
//
// Определяют внешний вид и поведение окна.
// Передаются в CreateWindow/CreateWindowEx.

pub const WS_OVERLAPPED: DWORD      = 0x00000000;
pub const WS_POPUP: DWORD           = 0x80000000;
pub const WS_CHILD: DWORD           = 0x40000000;
pub const WS_MINIMIZE: DWORD        = 0x20000000;
pub const WS_VISIBLE: DWORD         = 0x10000000;
pub const WS_DISABLED: DWORD        = 0x08000000;
pub const WS_CLIPSIBLINGS: DWORD    = 0x04000000;
pub const WS_CLIPCHILDREN: DWORD    = 0x02000000;
pub const WS_MAXIMIZE: DWORD        = 0x01000000;
pub const WS_CAPTION: DWORD         = 0x00C00000; // WS_BORDER | WS_DLGFRAME
pub const WS_BORDER: DWORD          = 0x00800000;
pub const WS_DLGFRAME: DWORD        = 0x00400000;
pub const WS_VSCROLL: DWORD         = 0x00200000;
pub const WS_HSCROLL: DWORD         = 0x00100000;
pub const WS_SYSMENU: DWORD         = 0x00080000;
pub const WS_THICKFRAME: DWORD      = 0x00040000;
pub const WS_MINIMIZEBOX: DWORD     = 0x00020000;
pub const WS_MAXIMIZEBOX: DWORD     = 0x00010000;

/// Стандартное окно с заголовком, кнопками закрытия/максимизации и рамкой.
pub const WS_OVERLAPPEDWINDOW: DWORD = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU
    | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;

// ---- Show Window (SW_*) ----
pub const SW_HIDE: DWORD            = 0;
pub const SW_SHOWNORMAL: DWORD      = 1;
pub const SW_SHOWMINIMIZED: DWORD   = 2;
pub const SW_SHOWMAXIMIZED: DWORD   = 3;
pub const SW_SHOW: DWORD            = 5;
pub const SW_MINIMIZE: DWORD        = 6;
pub const SW_RESTORE: DWORD         = 9;

// =============================================================================
// КЛАСС ОКНА (WNDCLASS)
// =============================================================================
//
// Перед созданием окна нужно зарегистрировать его "класс".
// Класс определяет общие свойства: оконную процедуру, иконку, курсор, фон.
//
// Стандартные классы Windows (не нужно регистрировать):
//   "Button"     — кнопки, checkbox, radio button
//   "Edit"       — текстовое поле
//   "Static"     — статический текст, картинка
//   "ListBox"    — список
//   "ComboBox"   — выпадающий список
//   "ScrollBar"  — полоса прокрутки
//   "#32770"     — диалоговое окно

/// ID зарегистрированного класса окна.
pub type ATOM = WORD;

/// Описание класса окна.
#[repr(C)]
pub struct WndClassW {
    pub style: DWORD,              // CS_HREDRAW | CS_VREDRAW и т.д.
    pub wnd_proc: u64,             // Адрес оконной процедуры (WndProc)
    pub cls_extra: i32,            // Доп. байты для класса
    pub wnd_extra: i32,            // Доп. байты для каждого окна
    pub instance: HANDLE,          // HINSTANCE модуля
    pub icon: HANDLE,              // Иконка (HICON)
    pub cursor: HANDLE,            // Курсор (HCURSOR)
    pub background: HANDLE,        // Фоновая кисть (HBRUSH)
    pub menu_name: LPCWSTR,        // Имя ресурса меню
    pub class_name: LPCWSTR,       // Имя класса (например "MyWindowClass")
}

// Class styles
pub const CS_HREDRAW: DWORD    = 0x0002; // Перерисовка при горизонтальном ресайзе
pub const CS_VREDRAW: DWORD    = 0x0001; // Перерисовка при вертикальном ресайзе
pub const CS_DBLCLKS: DWORD    = 0x0008; // Принимать двойные клики
pub const CS_OWNDC: DWORD      = 0x0020; // Уникальный DC для каждого окна

// =============================================================================
// GDI — Graphics Device Interface (каркас)
// =============================================================================
//
// GDI — слой 2D-графики Windows.
// Каждое рисование происходит через Device Context (DC):
//
//   HDC hdc = GetDC(hwnd);     // Получить DC окна
//   TextOut(hdc, 10, 10, "Hello", 5);  // Нарисовать текст
//   Rectangle(hdc, 0, 0, 100, 50);     // Нарисовать прямоугольник
//   ReleaseDC(hwnd, hdc);      // Освободить DC
//
// DC содержит "текущие" объекты:
//   - Pen (перо для линий)
//   - Brush (кисть для заливки)
//   - Font (шрифт для текста)
//   - Bitmap (поверхность рисования)
//   - Region (область отсечения)

/// Дескриптор Device Context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HDC(pub u64);

impl HDC {
    pub const NULL: HDC = HDC(0);
    pub fn is_null(self) -> bool { self.0 == 0 }
}

/// Дескриптор GDI объекта (Pen, Brush, Font, Bitmap, Region).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct HGDIOBJ(pub u64);

impl HGDIOBJ {
    pub const NULL: HGDIOBJ = HGDIOBJ(0);
}

/// Цвет в формате 0x00BBGGRR.
pub type COLORREF = DWORD;

/// Создать COLORREF из RGB.
pub const fn rgb(r: BYTE, g: BYTE, b: BYTE) -> COLORREF {
    (r as DWORD) | ((g as DWORD) << 8) | ((b as DWORD) << 16)
}

/// Извлечь компоненты из COLORREF.
pub fn get_r_value(c: COLORREF) -> BYTE { (c & 0xFF) as BYTE }
pub fn get_g_value(c: COLORREF) -> BYTE { ((c >> 8) & 0xFF) as BYTE }
pub fn get_b_value(c: COLORREF) -> BYTE { ((c >> 16) & 0xFF) as BYTE }

// Стандартные цвета
pub const CLR_BLACK: COLORREF   = rgb(0, 0, 0);
pub const CLR_WHITE: COLORREF   = rgb(255, 255, 255);
pub const CLR_RED: COLORREF     = rgb(255, 0, 0);
pub const CLR_GREEN: COLORREF   = rgb(0, 255, 0);
pub const CLR_BLUE: COLORREF    = rgb(0, 0, 255);
pub const CLR_YELLOW: COLORREF  = rgb(255, 255, 0);
pub const CLR_CYAN: COLORREF    = rgb(0, 255, 255);
pub const CLR_MAGENTA: COLORREF = rgb(255, 0, 255);
pub const CLR_GRAY: COLORREF    = rgb(128, 128, 128);

// =============================================================================
// КОНСОЛЬ
// =============================================================================
//
// Консольные приложения (IMAGE_SUBSYSTEM_WINDOWS_CUI) используют
// Console API: AllocConsole, WriteConsole, ReadConsole, SetConsoleTitle.
//
// В Windows консоль реализована через csrss.exe (Client/Server Runtime).
// В NoNameOS — через Console Server (user-space).

/// Режим консоли.
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum ConsoleMode {
    /// Обычный текстовый режим.
    Text = 0,
    /// VT100/ANSI escape sequences.
    VirtualTerminal = 1,
}

/// Атрибуты символа консоли (цвет текста + фон).
///
/// Формат: нижние 4 бита = цвет текста, верхние 4 бита = цвет фона.
pub type ConsoleAttribute = WORD;

// Цвета консоли
pub const FOREGROUND_BLUE: WORD      = 0x0001;
pub const FOREGROUND_GREEN: WORD     = 0x0002;
pub const FOREGROUND_RED: WORD       = 0x0004;
pub const FOREGROUND_INTENSITY: WORD = 0x0008;
pub const BACKGROUND_BLUE: WORD      = 0x0010;
pub const BACKGROUND_GREEN: WORD     = 0x0020;
pub const BACKGROUND_RED: WORD       = 0x0040;
pub const BACKGROUND_INTENSITY: WORD = 0x0080;

/// Координаты в консольном буфере.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct COORD {
    pub x: SHORT,
    pub y: SHORT,
}

/// Информация о консольном буфере экрана.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ConsoleScreenBufferInfo {
    pub size: COORD,              // Размер буфера
    pub cursor_position: COORD,   // Позиция курсора
    pub attributes: WORD,         // Текущие атрибуты
    pub window: RECT,             // Видимая область
    pub maximum_window_size: COORD,
}

// =============================================================================
// IPC ПРОТОКОЛ ДЛЯ WIN32 ПОДСИСТЕМЫ
// =============================================================================
//
// Когда приложение вызывает Win32 API (например CreateWindow),
// обёртка отправляет IPC-сообщение
// Win32 серверу с кодом операции.
//
// Сервер обрабатывает и возвращает результат.
//
// Коды операций для IPC с Win32 Server:

/// Операции Window Manager.
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum Win32Op {
    // Window Management
    RegisterClass    = 0x0001,
    CreateWindow     = 0x0002,
    DestroyWindow    = 0x0003,
    ShowWindow       = 0x0004,
    MoveWindow       = 0x0005,
    SetWindowText    = 0x0006,
    GetWindowText    = 0x0007,

    // Message Loop
    GetMessage       = 0x0010,
    PeekMessage      = 0x0011,
    SendMessage      = 0x0012,
    PostMessage      = 0x0013,
    TranslateMessage = 0x0014,
    DispatchMessage  = 0x0015,

    // GDI
    GetDC            = 0x0020,
    ReleaseDC        = 0x0021,
    BeginPaint       = 0x0022,
    EndPaint         = 0x0023,
    TextOut          = 0x0024,
    FillRect         = 0x0025,
    DrawRect         = 0x0026,

    // Console
    AllocConsole     = 0x0030,
    FreeConsole      = 0x0031,
    WriteConsole     = 0x0032,
    ReadConsole      = 0x0033,
    SetConsoleTitle  = 0x0034,
    SetConsoleCursorPosition = 0x0035,
}

// =============================================================================
// PROCESS ENVIRONMENT BLOCK
// =============================================================================
//
// PEB — структура в user-space памяти каждого процесса.
// Содержит информацию о процессе, доступную БЕЗ syscall:
//   - Адрес загрузки (ImageBase)
//   - Командная строка
//   - Переменные окружения
//   - Список загруженных DLL (LDR_DATA)
//   - Heap handle
//   - LastError value
//
// TEB — Thread Environment Block (один на каждый поток):
//   - Stack base/limit
//   - TLS (Thread Local Storage)
//   - LastError (GetLastError/SetLastError)
//   - PEB pointer
//
// Wine хранит PEB/TEB в пространстве процесса.
// ReactOS — аналогично.

/// Process Environment Block. (ранняя версия) 
#[repr(C)]
pub struct PEB {
    /// Флаг: процесс отлаживается (IsDebuggerPresent).
    pub being_debugged: BYTE,

    _reserved1: [BYTE; 7],

    /// Адрес загрузки основного модуля (ImageBase из PE).
    pub image_base_address: u64,

    /// Указатель на LDR_DATA (список загруженных DLL).
    pub ldr: u64,

    /// Указатель на RTL_USER_PROCESS_PARAMETERS.
    pub process_parameters: u64,

    /// Куча по умолчанию (ProcessHeap).
    pub process_heap: u64,
}

/// Thread Environment Block. (ранняя версия)
#[repr(C)]
pub struct TEB {
    /// Указатель на себя (для быстрого доступа через GS:[0x30]).
    pub self_ptr: u64,

    /// PEB процесса.
    pub peb: u64,

    /// ID текущего потока.
    pub thread_id: DWORD,

    /// ID процесса.
    pub process_id: DWORD,

    /// LastError (SetLastError/GetLastError).
    pub last_error: DWORD,

    /// TLS массив.
    pub tls_slots: [u64; 64],

    /// Стек.
    pub stack_base: u64,
    pub stack_limit: u64,
}

impl TEB {
    /// GetLastError() — читает last_error из TEB.
    pub fn get_last_error(&self) -> DWORD {
        self.last_error
    }

    /// SetLastError() — записывает last_error в TEB.
    pub fn set_last_error(&mut self, error: DWORD) {
        self.last_error = error;
    }
}
