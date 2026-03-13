// =============================================================================
// NoNameOS — Inter-Process Communication (IPC)
// =============================================================================
//
// IPC — СЕРДЦЕ микроядра. В монолитном ядре (Linux) компоненты вызывают
// друг друга напрямую (function call). В микроядре всё изолировано,
// и общение идёт через передачу сообщений.
//
// Зачем:
//   - Драйверы работают в user-space → нужно общаться с ядром и друг с другом
//   - Системные сервисы (Registry, SCM) — отдельные процессы
//   - Приложения (.exe) общаются с сервисами через IPC
//
// Наша модель IPC:
//
//   ┌──────────┐     сообщение      ┌──────────┐
//   │ Процесс A│ ──────────────────→│ Процесс B│
//   │ (клиент) │     endpoint       │ (сервер) │
//   └──────────┘                    └──────────┘
//
// Компоненты:
//
//   1. ENDPOINT (Конечная точка)
//      Именованный канал, через который можно отправлять/получать сообщения.
//      Сервер создаёт endpoint, клиент подключается к нему по имени.
//      Пример: endpoint "registry" — для обращений к реестру.
//
//   2. MESSAGE (Сообщение)
//      Фиксированная структура с полями:
//        - sender: кто послал (PID)
//        - msg_type: тип сообщения (request, response, notification)
//        - payload: данные (до 256 байт inline или shared memory)
//
//   3. PORT (Порт)
//      У каждого потока есть порт для приёма сообщений.
//      Поток может ждать на порту (блокируясь) или проверять (poll).
//
// Типы IPC:
//
//   СИНХРОННЫЙ (send + wait):
//     Клиент отправляет запрос и блокируется, пока не придёт ответ.
//     Простой и надёжный, но медленный для массовых операций.
//
//   АСИНХРОННЫЙ (send + poll):
//     Клиент отправляет и продолжает работу.
//     Проверяет ответ когда готов. Сложнее, но не блокирует.
//
//   SHARED MEMORY (для больших данных):
//     Два процесса мапят одну и ту же физическую страницу.
//     Данные не копируются — оба видят одну и ту же память.
//     Нужна синхронизация (мьютексы, семафоры).
//
// Пример вызова (будущий):
//
//   // Клиент хочет прочитать ключ реестра
//   let msg = Message::new(MSG_REGISTRY_READ, b"HKLM\\Software\\App");
//   let endpoint = ipc::connect("registry")?;
//   ipc::send(endpoint, &msg);
//   let response = ipc::receive(endpoint);  // блокируется до ответа
//   // response.payload содержит значение ключа
//
// Аналоги в реальных системах:
//   - Mach: ports + messages (macOS/iOS)
//   - L4: synchronous IPC (seL4, Fiasco)
//   - QNX: message passing (POSIX-совместимый RTOS)
//   - Windows NT: LPC/ALPC (Local Procedure Call)
//
// Мы начинаем с простого синхронного IPC, потом добавим async + shared memory.
// =============================================================================

use crate::task::Pid;

/// Максимальный размер payload в сообщении (inline, без shared memory).
pub const MAX_PAYLOAD: usize = 256;

/// Максимальное количество зарегистрированных endpoints.
const MAX_ENDPOINTS: usize = 64;

/// Максимальная длина имени endpoint.
const MAX_NAME_LEN: usize = 32;

// ---- Типы сообщений ----

/// Тип сообщения.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u32)]
pub enum MessageType {
    /// Запрос от клиента к серверу.
    Request = 1,

    /// Ответ от сервера клиенту.
    Response = 2,

    /// Уведомление (без ожидания ответа).
    Notification = 3,

    /// Ошибка.
    Error = 4,
}

// ---- Сообщение ----

/// IPC-сообщение — единица обмена данными между процессами.
#[repr(C)]
pub struct Message {
    /// PID отправителя (заполняется ядром автоматически).
    pub sender: Pid,

    /// Тип сообщения.
    pub msg_type: MessageType,

    /// Код операции (зависит от протокола endpoint'а).
    /// Например: 1 = READ, 2 = WRITE, 3 = OPEN, ...
    pub opcode: u32,

    /// Статус ответа (0 = OK, иначе код ошибки).
    pub status: i32,

    /// Размер данных в payload (0..MAX_PAYLOAD).
    pub payload_len: usize,

    /// Данные сообщения.
    pub payload: [u8; MAX_PAYLOAD],
}

impl Message {
    /// Создать пустое сообщение.
    pub const fn empty() -> Self {
        Message {
            sender: 0,
            msg_type: MessageType::Request,
            opcode: 0,
            status: 0,
            payload_len: 0,
            payload: [0; MAX_PAYLOAD],
        }
    }

    /// Создать запрос с данными.
    pub fn request(opcode: u32, data: &[u8]) -> Self {
        let mut msg = Message::empty();
        msg.msg_type = MessageType::Request;
        msg.opcode = opcode;
        let len = core::cmp::min(data.len(), MAX_PAYLOAD);
        msg.payload[..len].copy_from_slice(&data[..len]);
        msg.payload_len = len;
        msg
    }

    /// Создать ответ.
    pub fn response(opcode: u32, status: i32, data: &[u8]) -> Self {
        let mut msg = Message::empty();
        msg.msg_type = MessageType::Response;
        msg.opcode = opcode;
        msg.status = status;
        let len = core::cmp::min(data.len(), MAX_PAYLOAD);
        msg.payload[..len].copy_from_slice(&data[..len]);
        msg.payload_len = len;
        msg
    }
}

// ---- Endpoint ----

/// Endpoint — именованный канал для IPC.
///
/// Сервер регистрирует endpoint, клиент подключается по имени.
/// Сообщения складываются в очередь endpoint'а.
struct Endpoint {
    /// Имя endpoint'а (например, "registry", "display", "audio").
    name: [u8; MAX_NAME_LEN],
    name_len: usize,

    /// PID процесса-владельца (сервера).
    owner: Pid,

    /// Активен ли endpoint.
    active: bool,

    /// Очередь сообщений (простой кольцевой буфер).
    queue: [Message; 16],
    queue_read: usize,
    queue_write: usize,
    queue_count: usize,
}

impl Endpoint {
    const QUEUE_SIZE: usize = 16;

    const fn empty() -> Self {
        Endpoint {
            name: [0; MAX_NAME_LEN],
            name_len: 0,
            owner: 0,
            active: false,
            queue: [const { Message::empty() }; 16],
            queue_read: 0,
            queue_write: 0,
            queue_count: 0,
        }
    }

    /// Положить сообщение в очередь. false если очередь полна.
    fn enqueue(&mut self, msg: Message) -> bool {
        if self.queue_count >= Self::QUEUE_SIZE {
            return false;
        }
        self.queue[self.queue_write] = msg;
        self.queue_write = (self.queue_write + 1) % Self::QUEUE_SIZE;
        self.queue_count += 1;
        true
    }

    /// Достать сообщение из очереди.
    fn dequeue(&mut self) -> Option<&Message> {
        if self.queue_count == 0 {
            return None;
        }
        let idx = self.queue_read;
        self.queue_read = (self.queue_read + 1) % Self::QUEUE_SIZE;
        self.queue_count -= 1;
        Some(&self.queue[idx])
    }
}

// ---- Глобальный реестр endpoint'ов ----

static mut ENDPOINTS: [Endpoint; MAX_ENDPOINTS] = [const { Endpoint::empty() }; MAX_ENDPOINTS];

// ---- Публичный API ----

/// Зарегистрировать новый endpoint (вызывается сервером).
///
/// Возвращает ID endpoint'а или None если все заняты.
pub fn register_endpoint(name: &str, owner: Pid) -> Option<usize> {
    let name_bytes = name.as_bytes();
    if name_bytes.len() > MAX_NAME_LEN {
        return None;
    }

    unsafe {
        for i in 0..MAX_ENDPOINTS {
            if !ENDPOINTS[i].active {
                ENDPOINTS[i].active = true;
                ENDPOINTS[i].owner = owner;
                ENDPOINTS[i].name_len = name_bytes.len();
                ENDPOINTS[i].name[..name_bytes.len()].copy_from_slice(name_bytes);
                return Some(i);
            }
        }
    }
    None
}

/// Найти endpoint по имени.
pub fn find_endpoint(name: &str) -> Option<usize> {
    let name_bytes = name.as_bytes();
    unsafe {
        for i in 0..MAX_ENDPOINTS {
            if ENDPOINTS[i].active
                && ENDPOINTS[i].name_len == name_bytes.len()
                && &ENDPOINTS[i].name[..name_bytes.len()] == name_bytes
            {
                return Some(i);
            }
        }
    }
    None
}

/// Отправить сообщение в endpoint.
pub fn send(endpoint_id: usize, msg: Message) -> Result<(), &'static str> {
    unsafe {
        if endpoint_id >= MAX_ENDPOINTS || !ENDPOINTS[endpoint_id].active {
            return Err("invalid endpoint");
        }
        if !ENDPOINTS[endpoint_id].enqueue(msg) {
            return Err("queue full");
        }
    }
    Ok(())
}

/// Получить сообщение из endpoint (неблокирующий).
/// В будущем добавим блокирующий receive (с засыпанием потока).
pub fn receive(endpoint_id: usize) -> Option<Message> {
    unsafe {
        if endpoint_id >= MAX_ENDPOINTS || !ENDPOINTS[endpoint_id].active {
            return None;
        }
        // Копируем сообщение, чтобы вернуть по значению
        ENDPOINTS[endpoint_id].dequeue().map(|msg| {
            let mut copy = Message::empty();
            copy.sender = msg.sender;
            copy.msg_type = msg.msg_type;
            copy.opcode = msg.opcode;
            copy.status = msg.status;
            copy.payload_len = msg.payload_len;
            copy.payload.copy_from_slice(&msg.payload);
            copy
        })
    }
}

/// Удалить endpoint (вызывается при завершении сервера).
pub fn unregister_endpoint(endpoint_id: usize) {
    unsafe {
        if endpoint_id < MAX_ENDPOINTS {
            ENDPOINTS[endpoint_id].active = false;
        }
    }
}
