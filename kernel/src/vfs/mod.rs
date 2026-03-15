// =============================================================================
// NoNameOS — Virtual File System (VFS)
// =============================================================================
//
// VFS — единый интерфейс для всех файловых систем.
// Приложения вызывают open(), read(), write(), close() — VFS направляет
// вызовы в конкретную ФС (ramfs, ext4, NTFS, FAT32...).
//
// Архитектура (аналогична Linux VFS):
//
//   ┌──────────────┐
//   │  Приложение   │  open("/mnt/disk/file.txt", O_RDONLY)
//   └──────┬───────┘
//          │ syscall
//   ┌──────▼───────┐
//   │     VFS      │  path lookup → dentry → inode → file
//   └──────┬───────┘
//          │
//   ┌──────▼───────┐
//   │  Конкретная   │  ramfs, ext4, fat32, ntfs...
//   │  файловая     │  Реализует file_operations + inode_operations
//   │  система      │
//   └──────────────┘
//
// Ключевые структуры:
//
//   SUPERBLOCK — экземпляр смонтированной файловой системы.
//     Содержит: тип ФС, корневой inode, операции.
//     Аналог: struct super_block в Linux.
//
//   INODE — метаданные файла/директории.
//     Содержит: размер, тип (файл/dir/symlink), права, указатели на данные.
//     Аналог: struct inode в Linux.
//     Один inode = один файл/директория. Имя хранится в dentry, НЕ в inode.
//
//   DENTRY — запись в директории (связь имя ↔ inode).
//     Содержит: имя файла, ссылка на inode, ссылка на родительскую dentry.
//     Аналог: struct dentry в Linux.
//     Кэшируется для быстрого path lookup.
//
//   FILE — открытый файл (дескриптор).
//     Содержит: ссылка на inode, текущая позиция (offset), режим доступа.
//     Аналог: struct file в Linux.
//     У каждого процесса — своя таблица открытых файлов.
//
// Path lookup (как VFS находит файл по пути):
//
//   open("/home/user/test.txt")
//   1. Начинаем с root dentry (/)
//   2. Ищем dentry "home" среди дочерних → находим inode директории
//   3. В inode "home" ищем dentry "user" → inode директории
//   4. В inode "user" ищем dentry "test.txt" → inode файла
//   5. Создаём struct File с этим inode → возвращаем fd
//
// Mount:
//   mount("/dev/sda1", "/mnt/disk", "ext4")
//   Создаёт superblock для ext4, привязывает его к dentry "/mnt/disk".
//   Все обращения внутрь /mnt/disk/ теперь идут через ext4.
//
// Для Win32 совместимости:
//   CreateFile("C:\\Windows\\notepad.exe") будет транслироваться в:
//   open("/drives/c/Windows/notepad.exe")
//   Диски C:, D: — это mount points в нашей VFS.
//
// Источники:
//   - Linux: fs/namei.c (path lookup), fs/open.c, fs/read_write.c
//   - "Understanding the Linux Kernel" — Chapter 12: VFS
//   - OSDev wiki: VFS
// =============================================================================

pub mod ramfs;

// ---- Типы ----

/// Тип записи в файловой системе.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum InodeType {
    File      = 1,  // Обычный файл
    Directory = 2,  // Директория
    Symlink   = 3,  // Символическая ссылка
    Device    = 4,  // Файл устройства (/dev/sda, /dev/gpu0)
    Pipe      = 5,  // Именованный канал (FIFO)
}

/// Режим доступа при открытии файла.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u32)]
pub enum OpenMode {
    ReadOnly  = 0,
    WriteOnly = 1,
    ReadWrite = 2,
    Append    = 4,
    Create    = 8,
    Truncate  = 16,
}

/// Номер inode (уникален в пределах одной ФС).
pub type InodeNum = u64;

/// Файловый дескриптор.
pub type Fd = i32;

// ---- Максимумы ----

const MAX_SUPERBLOCKS: usize = 8;
const MAX_INODES: usize = 256;
const MAX_DENTRIES: usize = 256;
const MAX_OPEN_FILES: usize = 128;
const MAX_NAME_LEN: usize = 64;
const MAX_PATH_LEN: usize = 256;

// ---- Операции файловой системы (function pointers) ----

/// Операции суперблока — создание/удаление inode.
/// Реализуются конкретной файловой системой.
pub struct SuperOps {
    /// Создать новый inode.
    pub alloc_inode: fn(sb_idx: usize, itype: InodeType) -> Option<usize>,

    /// Удалить inode (когда refcount = 0).
    pub free_inode: fn(sb_idx: usize, inode_idx: usize),
}

/// Операции над inode — lookup, create, mkdir.
pub struct InodeOps {
    /// Найти дочерний элемент по имени в директории.
    /// Возвращает индекс dentry или None.
    pub lookup: fn(dir_inode: usize, name: &[u8]) -> Option<usize>,

    /// Создать файл в директории.
    pub create: fn(dir_inode: usize, name: &[u8], itype: InodeType) -> Option<usize>,
}

/// Операции над открытым файлом — read, write, seek.
pub struct FileOps {
    /// Прочитать данные из файла.
    /// Возвращает количество прочитанных байт.
    pub read: fn(inode_idx: usize, offset: usize, buf: &mut [u8]) -> isize,

    /// Записать данные в файл.
    /// Возвращает количество записанных байт.
    pub write: fn(inode_idx: usize, offset: usize, data: &[u8]) -> isize,
}

// Заглушки по умолчанию
fn null_alloc_inode(_sb: usize, _t: InodeType) -> Option<usize> { None }
fn null_free_inode(_sb: usize, _i: usize) {}
fn null_lookup(_dir: usize, _name: &[u8]) -> Option<usize> { None }
fn null_create(_dir: usize, _name: &[u8], _t: InodeType) -> Option<usize> { None }
fn null_read(_i: usize, _off: usize, _buf: &mut [u8]) -> isize { 0 }
fn null_write(_i: usize, _off: usize, _data: &[u8]) -> isize { 0 }

// ---- Superblock ----

/// Суперблок — экземпляр смонтированной файловой системы.
pub struct Superblock {
    /// Имя типа ФС ("ramfs", "ext4", "fat32").
    pub fs_type: [u8; 16],
    pub fs_type_len: usize,

    /// Активен ли суперблок.
    pub active: bool,

    /// Индекс корневого inode этой ФС.
    pub root_inode: usize,

    /// Операции.
    pub ops: SuperOps,
}

impl Superblock {
    pub const fn empty() -> Self {
        Superblock {
            fs_type: [0; 16],
            fs_type_len: 0,
            active: false,
            root_inode: 0,
            ops: SuperOps {
                alloc_inode: null_alloc_inode,
                free_inode: null_free_inode,
            },
        }
    }
}

// ---- Inode ----

/// Inode — метаданные файла/директории.
pub struct Inode {
    /// Номер inode (уникален в пределах ФС).
    pub ino: InodeNum,

    /// Тип (файл, директория, ...).
    pub itype: InodeType,

    /// Размер файла в байтах.
    pub size: usize,

    /// Индекс суперблока (к какой ФС принадлежит).
    pub sb_idx: usize,

    /// Активен ли inode.
    pub active: bool,

    /// Счётчик ссылок (сколько dentry указывают сюда).
    pub nlink: u32,

    /// Операции.
    pub inode_ops: InodeOps,
    pub file_ops: FileOps,

    /// Данные файла (inline для ramfs, указатель для дисковых ФС).
    /// Для ramfs: данные хранятся прямо тут (до 4096 байт).
    /// Для дисковых ФС: тут будут block pointers.
    pub data_page: usize,  // физический адрес страницы данных (0 = нет)
    pub data_size: usize,  // сколько данных реально записано
}

impl Inode {
    pub const fn empty() -> Self {
        Inode {
            ino: 0,
            itype: InodeType::File,
            size: 0,
            sb_idx: 0,
            active: false,
            nlink: 0,
            inode_ops: InodeOps {
                lookup: null_lookup,
                create: null_create,
            },
            file_ops: FileOps {
                read: null_read,
                write: null_write,
            },
            data_page: 0,
            data_size: 0,
        }
    }
}

// ---- Dentry ----

/// Dentry — запись в директории (имя → inode).
pub struct Dentry {
    /// Имя файла/директории.
    pub name: [u8; MAX_NAME_LEN],
    pub name_len: usize,

    /// Индекс inode, на который указывает.
    pub inode_idx: usize,

    /// Индекс родительской dentry (-1 для корня).
    pub parent_idx: i32,

    /// Активна ли запись.
    pub active: bool,

    /// Индекс суперблока (может измениться при mount point).
    pub sb_idx: usize,
}

impl Dentry {
    pub const fn empty() -> Self {
        Dentry {
            name: [0; MAX_NAME_LEN],
            name_len: 0,
            inode_idx: 0,
            parent_idx: -1,
            active: false,
            sb_idx: 0,
        }
    }

    pub fn set_name(&mut self, n: &[u8]) {
        let len = if n.len() > MAX_NAME_LEN - 1 { MAX_NAME_LEN - 1 } else { n.len() };
        self.name[..len].copy_from_slice(&n[..len]);
        self.name[len] = 0;
        self.name_len = len;
    }

    pub fn name_eq(&self, n: &[u8]) -> bool {
        if self.name_len != n.len() { return false; }
        &self.name[..self.name_len] == n
    }
}

// ---- File (открытый файл) ----

/// Открытый файл — привязан к процессу.
pub struct File {
    /// Индекс inode.
    pub inode_idx: usize,

    /// Текущая позиция чтения/записи.
    pub offset: usize,

    /// Режим доступа.
    pub mode: u32,

    /// Активен ли дескриптор.
    pub active: bool,
}

impl File {
    pub const fn empty() -> Self {
        File {
            inode_idx: 0,
            offset: 0,
            mode: 0,
            active: false,
        }
    }
}

// ---- Глобальные таблицы ----

static mut SUPERBLOCKS: [Superblock; MAX_SUPERBLOCKS] = [const { Superblock::empty() }; MAX_SUPERBLOCKS];
static mut INODES: [Inode; MAX_INODES] = [const { Inode::empty() }; MAX_INODES];
static mut DENTRIES: [Dentry; MAX_DENTRIES] = [const { Dentry::empty() }; MAX_DENTRIES];
static mut FILES: [File; MAX_OPEN_FILES] = [const { File::empty() }; MAX_OPEN_FILES];

static mut NEXT_INO: InodeNum = 1;

// ---- Внутренние хелперы ----

/// Найти свободный слот в таблице supeblocks.
pub fn alloc_superblock() -> Option<usize> {
    unsafe {
        for i in 0..MAX_SUPERBLOCKS {
            if !SUPERBLOCKS[i].active {
                return Some(i);
            }
        }
    }
    None
}

/// Получить ссылку на суперблок.
pub fn get_superblock(idx: usize) -> Option<&'static Superblock> {
    unsafe {
        if idx < MAX_SUPERBLOCKS && SUPERBLOCKS[idx].active {
            Some(&SUPERBLOCKS[idx])
        } else {
            None
        }
    }
}

/// Получить мутабельную ссылку на суперблок.
pub fn get_superblock_mut(idx: usize) -> Option<&'static mut Superblock> {
    unsafe {
        if idx < MAX_SUPERBLOCKS && SUPERBLOCKS[idx].active {
            Some(&mut SUPERBLOCKS[idx])
        } else {
            None
        }
    }
}

/// Выделить inode в глобальной таблице.
pub fn alloc_inode_slot() -> Option<usize> {
    unsafe {
        for i in 0..MAX_INODES {
            if !INODES[i].active {
                INODES[i].active = true;
                INODES[i].ino = NEXT_INO;
                NEXT_INO += 1;
                return Some(i);
            }
        }
    }
    None
}

/// Получить ссылку на inode.
pub fn get_inode(idx: usize) -> Option<&'static Inode> {
    unsafe {
        if idx < MAX_INODES && INODES[idx].active {
            Some(&INODES[idx])
        } else {
            None
        }
    }
}

/// Получить мутабельную ссылку на inode.
pub fn get_inode_mut(idx: usize) -> Option<&'static mut Inode> {
    unsafe {
        if idx < MAX_INODES && INODES[idx].active {
            Some(&mut INODES[idx])
        } else {
            None
        }
    }
}

/// Выделить dentry.
pub fn alloc_dentry() -> Option<usize> {
    unsafe {
        for i in 0..MAX_DENTRIES {
            if !DENTRIES[i].active {
                return Some(i);
            }
        }
    }
    None
}

/// Получить ссылку на dentry.
pub fn get_dentry(idx: usize) -> Option<&'static Dentry> {
    unsafe {
        if idx < MAX_DENTRIES && DENTRIES[idx].active {
            Some(&DENTRIES[idx])
        } else {
            None
        }
    }
}

/// Получить мутабельную ссылку на dentry.
pub fn get_dentry_mut(idx: usize) -> Option<&'static mut Dentry> {
    unsafe {
        if idx < MAX_DENTRIES && DENTRIES[idx].active {
            Some(&mut DENTRIES[idx])
        } else {
            None
        }
    }
}

// ---- Path Lookup ----

/// Разрезолвить путь: "/home/user/file.txt" → индекс dentry.
///
/// Начинаем с root dentry (индекс 0) и идём по компонентам.
pub fn path_lookup(path: &[u8]) -> Option<usize> {
    if path.is_empty() { return None; }

    // Пропускаем начальный '/'
    let start = if path[0] == b'/' { 1 } else { 0 };
    if start >= path.len() {
        // Путь "/" → root dentry
        return Some(0);
    }

    let mut current_dentry = 0usize; // начинаем с root

    let mut pos = start;
    while pos < path.len() {
        // Найти конец текущего компонента
        let mut end = pos;
        while end < path.len() && path[end] != b'/' {
            end += 1;
        }

        if end == pos {
            pos = end + 1;
            continue;
        }

        let component = &path[pos..end];

        // Ищем дочернюю dentry с таким именем
        let found = find_child_dentry(current_dentry, component);
        match found {
            Some(child_idx) => {
                current_dentry = child_idx;
            }
            None => return None, // компонент не найден
        }

        pos = end + 1;
    }

    Some(current_dentry)
}

/// Найти дочернюю dentry по имени внутри директории.
fn find_child_dentry(parent_dentry_idx: usize, name: &[u8]) -> Option<usize> {
    unsafe {
        for i in 0..MAX_DENTRIES {
            if DENTRIES[i].active
                && DENTRIES[i].parent_idx == parent_dentry_idx as i32
                && DENTRIES[i].name_eq(name)
            {
                return Some(i);
            }
        }
    }
    None
}

// ---- Публичный API (системные вызовы) ----

/// Открыть файл по пути.
/// Возвращает индекс в таблице файлов (будущий fd).
pub fn open(path: &[u8], mode: u32) -> Option<usize> {
    let dentry_idx = path_lookup(path)?;
    let dentry = get_dentry(dentry_idx)?;
    let _inode = get_inode(dentry.inode_idx)?;

    // Найти свободный слот в таблице файлов
    unsafe {
        for i in 0..MAX_OPEN_FILES {
            if !FILES[i].active {
                FILES[i].active = true;
                FILES[i].inode_idx = dentry.inode_idx;
                FILES[i].offset = 0;
                FILES[i].mode = mode;
                return Some(i);
            }
        }
    }
    None
}

/// Прочитать из открытого файла.
pub fn read(fd: usize, buf: &mut [u8]) -> isize {
    unsafe {
        if fd >= MAX_OPEN_FILES || !FILES[fd].active { return -1; }
        let inode_idx = FILES[fd].inode_idx;
        let offset = FILES[fd].offset;

        if inode_idx >= MAX_INODES || !INODES[inode_idx].active { return -1; }

        let bytes_read = (INODES[inode_idx].file_ops.read)(inode_idx, offset, buf);
        if bytes_read > 0 {
            FILES[fd].offset += bytes_read as usize;
        }
        bytes_read
    }
}

/// Записать в открытый файл.
pub fn write(fd: usize, data: &[u8]) -> isize {
    unsafe {
        if fd >= MAX_OPEN_FILES || !FILES[fd].active { return -1; }
        let inode_idx = FILES[fd].inode_idx;
        let offset = FILES[fd].offset;

        if inode_idx >= MAX_INODES || !INODES[inode_idx].active { return -1; }

        let bytes_written = (INODES[inode_idx].file_ops.write)(inode_idx, offset, data);
        if bytes_written > 0 {
            FILES[fd].offset += bytes_written as usize;
        }
        bytes_written
    }
}

/// Закрыть файл.
pub fn close(fd: usize) {
    unsafe {
        if fd < MAX_OPEN_FILES {
            FILES[fd].active = false;
        }
    }
}

/// Установить позицию чтения/записи.
pub fn seek(fd: usize, offset: usize) -> bool {
    unsafe {
        if fd >= MAX_OPEN_FILES || !FILES[fd].active { return false; }
        FILES[fd].offset = offset;
        true
    }
}

/// Создать файл/директорию в указанном пути.
/// `parent_path` — путь к родительской директории.
/// `name` — имя нового файла.
pub fn create(parent_path: &[u8], name: &[u8], itype: InodeType) -> Option<usize> {
    let parent_dentry_idx = path_lookup(parent_path)?;
    let parent_dentry = get_dentry(parent_dentry_idx)?;
    let parent_inode_idx = parent_dentry.inode_idx;
    let parent_inode = get_inode(parent_inode_idx)?;

    // Проверяем, что родитель — директория
    if parent_inode.itype != InodeType::Directory {
        return None;
    }

    // Вызываем create операцию inode
    let new_dentry_idx = (parent_inode.inode_ops.create)(parent_inode_idx, name, itype)?;
    Some(new_dentry_idx)
}

/// Инициализация VFS.
/// Создаёт root dentry и монтирует ramfs как корневую ФС.
pub fn init() {
    // Инициализируем ramfs как корневую файловую систему
    ramfs::init();
}
