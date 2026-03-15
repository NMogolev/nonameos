// =============================================================================
// NoNameOS — RAM Filesystem (ramfs)
// =============================================================================
//
// ramfs — простейшая файловая система, хранящая всё в оперативной памяти.
// Используется как корневая ФС при загрузке, пока не смонтирована дисковая.
//
// В Linux ramfs — тоже первая ФС. Потом поверх неё монтируется initramfs,
// из которого загружаются модули ядра, а потом pivot_root на настоящий корень.
//
// Наша ramfs:
//   - Файлы хранят данные в физических страницах (по 4 KiB на файл)
//   - Директории — это inode с типом Directory
//   - Поиск по имени — линейный перебор dentry (для начала ОК)
//   - Максимальный размер файла = PAGE_SIZE (4096 байт)
//     TODO: multi-page файлы через linked list / extent tree
//
// Структура при загрузке:
//
//   /                     ← корень (ramfs)
//   ├── dev/              ← устройства (будущее: devfs)
//   ├── sys/              ← системная информация (будущее: sysfs)
//   ├── proc/             ← информация о процессах (будущее: procfs)
//   ├── mnt/              ← точки монтирования для дисков
//   │   ├── c/            ← будущий C: диск (для Win32 совместимости)
//   │   └── d/
//   └── tmp/              ← временные файлы
//
// Источники:
//   - Linux: fs/ramfs/inode.c, fs/ramfs/file-mmu.c
//   - "Linux Kernel Development" by Robert Love — Chapter 13
// =============================================================================

use crate::memory::{phys, PAGE_SIZE};
use super::*;

// ---- ramfs операции ----

/// Создать inode для ramfs.
fn ramfs_alloc_inode(sb_idx: usize, itype: InodeType) -> Option<usize> {
    let idx = alloc_inode_slot()?;
    let inode = get_inode_mut(idx)?;

    inode.itype = itype;
    inode.size = 0;
    inode.sb_idx = sb_idx;
    inode.nlink = 1;
    inode.data_page = 0;
    inode.data_size = 0;

    // Устанавливаем операции в зависимости от типа
    match itype {
        InodeType::Directory => {
            inode.inode_ops = InodeOps {
                lookup: ramfs_lookup,
                create: ramfs_create,
            };
            inode.file_ops = FileOps {
                read: null_read,
                write: null_write,
            };
        }
        InodeType::File => {
            inode.inode_ops = InodeOps {
                lookup: null_lookup,
                create: null_create,
            };
            inode.file_ops = FileOps {
                read: ramfs_read,
                write: ramfs_write,
            };
        }
        _ => {}
    }

    Some(idx)
}

/// Освободить inode (и связанную страницу данных).
fn ramfs_free_inode(_sb_idx: usize, inode_idx: usize) {
    if let Some(inode) = get_inode_mut(inode_idx) {
        if inode.data_page != 0 {
            phys::free_frame(inode.data_page);
            inode.data_page = 0;
        }
        inode.active = false;
    }
}

/// Lookup: найти дочерний элемент в директории по имени.
///
/// Перебираем все dentry, чей parent = dir_inode.
/// (Это O(n), но для ramfs с малым числом файлов — нормально.)
fn ramfs_lookup(dir_inode_idx: usize, name: &[u8]) -> Option<usize> {
    // Находим dentry директории, потом ищем дочернюю dentry
    unsafe {
        // Сначала найдём dentry, указывающую на dir_inode
        let mut dir_dentry_idx = None;
        for i in 0..256 { // MAX_DENTRIES
            let d = get_dentry(i);
            if let Some(d) = d {
                if d.inode_idx == dir_inode_idx {
                    dir_dentry_idx = Some(i);
                    break;
                }
            }
        }

        let parent_idx = dir_dentry_idx? as i32;

        // Теперь ищем дочернюю dentry с нужным именем
        for i in 0..256 {
            if let Some(d) = get_dentry(i) {
                if d.parent_idx == parent_idx && d.name_eq(name) {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Create: создать новый файл/директорию внутри директории.
fn ramfs_create(dir_inode_idx: usize, name: &[u8], itype: InodeType) -> Option<usize> {
    let dir_inode = get_inode(dir_inode_idx)?;
    let sb_idx = dir_inode.sb_idx;

    // Создаём новый inode
    let new_inode_idx = ramfs_alloc_inode(sb_idx, itype)?;

    // Если файл — выделяем страницу для данных
    if itype == InodeType::File {
        if let Some(page) = phys::alloc_frame() {
            // Обнуляем страницу
            let ptr = page as *mut u8;
            unsafe {
                core::ptr::write_bytes(ptr, 0, PAGE_SIZE);
            }
            if let Some(inode) = get_inode_mut(new_inode_idx) {
                inode.data_page = page;
            }
        }
    }

    // Находим dentry родительской директории
    let mut parent_dentry_idx: i32 = -1;
    for i in 0..256 {
        if let Some(d) = get_dentry(i) {
            if d.inode_idx == dir_inode_idx {
                parent_dentry_idx = i as i32;
                break;
            }
        }
    }

    // Создаём dentry для нового элемента
    let dentry_idx = alloc_dentry()?;
    let dentry = get_dentry_mut(dentry_idx)?;
    dentry.set_name(name);
    dentry.inode_idx = new_inode_idx;
    dentry.parent_idx = parent_dentry_idx;
    dentry.active = true;
    dentry.sb_idx = sb_idx;

    Some(dentry_idx)
}

/// Read: прочитать данные из файла.
fn ramfs_read(inode_idx: usize, offset: usize, buf: &mut [u8]) -> isize {
    let inode = match get_inode(inode_idx) {
        Some(i) => i,
        None => return -1,
    };

    if inode.data_page == 0 {
        return 0; // нет данных
    }

    if offset >= inode.data_size {
        return 0; // за пределами файла
    }

    let available = inode.data_size - offset;
    let to_read = if buf.len() < available { buf.len() } else { available };

    let src = (inode.data_page + offset) as *const u8;
    unsafe {
        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), to_read);
    }

    to_read as isize
}

/// Write: записать данные в файл.
fn ramfs_write(inode_idx: usize, offset: usize, data: &[u8]) -> isize {
    let inode = match get_inode_mut(inode_idx) {
        Some(i) => i,
        None => return -1,
    };

    // Если страница ещё не выделена — выделяем
    if inode.data_page == 0 {
        match phys::alloc_frame() {
            Some(page) => {
                unsafe { core::ptr::write_bytes(page as *mut u8, 0, PAGE_SIZE); }
                inode.data_page = page;
            }
            None => return -1, // нет памяти
        }
    }

    // Проверяем, что не выходим за пределы страницы
    if offset >= PAGE_SIZE {
        return 0;
    }

    let max_write = PAGE_SIZE - offset;
    let to_write = if data.len() < max_write { data.len() } else { max_write };

    let dst = (inode.data_page + offset) as *mut u8;
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), dst, to_write);
    }

    // Обновляем размер файла
    let new_end = offset + to_write;
    if new_end > inode.data_size {
        inode.data_size = new_end;
        inode.size = new_end;
    }

    to_write as isize
}

// ---- Инициализация ----

/// Инициализация ramfs как корневой файловой системы.
///
/// Создаёт:
///   - Суперблок для ramfs
///   - Корневой inode (директория /)
///   - Корневую dentry
///   - Базовые директории: /dev, /sys, /proc, /mnt, /tmp
pub fn init() {
    // 1. Создаём суперблок
    let sb_idx = match alloc_superblock() {
        Some(i) => i,
        None => return,
    };

    unsafe {
        let sb = get_superblock_mut(sb_idx).unwrap();
        let name = b"ramfs";
        sb.fs_type[..name.len()].copy_from_slice(name);
        sb.fs_type_len = name.len();
        sb.active = true;
        sb.ops = SuperOps {
            alloc_inode: ramfs_alloc_inode,
            free_inode: ramfs_free_inode,
        };
    }

    // 2. Создаём корневой inode (директория /)
    let root_inode_idx = match ramfs_alloc_inode(sb_idx, InodeType::Directory) {
        Some(i) => i,
        None => return,
    };

    unsafe {
        if let Some(sb) = get_superblock_mut(sb_idx) {
            sb.root_inode = root_inode_idx;
        }
    }

    // 3. Создаём корневую dentry (индекс 0)
    let root_dentry_idx = match alloc_dentry() {
        Some(i) => i,
        None => return,
    };

    if let Some(d) = get_dentry_mut(root_dentry_idx) {
        d.set_name(b"/");
        d.inode_idx = root_inode_idx;
        d.parent_idx = -1; // корень, нет родителя
        d.active = true;
        d.sb_idx = sb_idx;
    }

    // 4. Создаём базовые директории
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"dev");
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"sys");
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"proc");
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"mnt");
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"tmp");
    create_subdir(root_inode_idx, root_dentry_idx, sb_idx, b"drives");
}

/// Создать поддиректорию.
fn create_subdir(
    parent_inode_idx: usize,
    parent_dentry_idx: usize,
    sb_idx: usize,
    name: &[u8],
) {
    // Создаём inode для директории
    let inode_idx = match ramfs_alloc_inode(sb_idx, InodeType::Directory) {
        Some(i) => i,
        None => return,
    };

    // Создаём dentry
    let dentry_idx = match alloc_dentry() {
        Some(i) => i,
        None => return,
    };

    if let Some(d) = get_dentry_mut(dentry_idx) {
        d.set_name(name);
        d.inode_idx = inode_idx;
        d.parent_idx = parent_dentry_idx as i32;
        d.active = true;
        d.sb_idx = sb_idx;
    }
}
