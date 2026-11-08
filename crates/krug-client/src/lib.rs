//! Сетевой клиент «Круга»: TCP для своего сервера и WebSocket для
//! бесплатных хостингов вроде Render, где наружу отдаётся только HTTP(S).
//!
//! Один фоновый поток держит соединение, переподключается с backoff и
//! складывает состояние в `Arc<Mutex<State>>`, который читает интерфейс.

use std::collections::HashMap;
use std::io::{self, Read};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use krug_core::frame::write_frame;
use krug_core::model::{Message, Room, User};
use krug_core::{
    ClientKind, ClientOp, ErrorCode, ServerOp, MAX_FRAME, MAX_STICKER, PROTOCOL_VERSION,
};

const POLL: Duration = Duration::from_millis(100);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(50);
const PING_EVERY: Duration = Duration::from_secs(10);

/// Состояние соединения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Online,
    Offline,
    /// Сервер отказал: неверный пароль, инвайт или версия.
    Rejected,
}

/// Кто печатает.
#[derive(Debug, Clone)]
pub struct TypingUser {
    pub id: u64,
    pub name: String,
    pub since: Instant,
}

/// Активный или входящий звонок.
#[derive(Debug, Clone)]
pub struct CallState {
    pub peer: User,
    pub incoming: bool,
    pub active: bool,
    pub started_ms: u64,
    pub muted: bool,
}

/// Всё, что нужно интерфейсу. Клонируется дёшево (история ограничена).
#[derive(Debug, Clone)]
pub struct State {
    pub status: Status,
    pub error: Option<String>,
    pub server_name: String,
    pub motd: String,
    pub my_name: String,
    pub me: Option<u64>,
    pub rooms: Vec<Room>,
    pub current: Option<u64>,
    pub online: u32,
    pub messages: Vec<Message>,
    pub members: Vec<User>,
    pub users: Vec<User>,
    pub reads: HashMap<u64, u64>,
    pub typing: Vec<TypingUser>,
    pub unread: HashMap<u64, u32>,
    pub call: Option<CallState>,
    pub voice: Vec<(u64, Vec<u8>)>,
    pub last_seq: u64,
    pub joined: bool,
    pub history_complete: bool,
    pub latency_ms: Option<u32>,
    /// Когда пришла последняя ошибка (мс Unix) — для всплывающей плашки.
    pub error_at: u64,
    /// Счётчик меняется при обновлении списка комнат — GUI ловит новые.
    pub rooms_version: u64,
    pub transport: &'static str,
    pub addr: String,
}

impl State {
    pub fn new(name: &str, addr: &str) -> State {
        State {
            status: Status::Connecting,
            error: None,
            server_name: "Круг".into(),
            motd: String::new(),
            my_name: name.to_string(),
            me: None,
            rooms: Vec::new(),
            current: None,
            online: 0,
            messages: Vec::new(),
            members: Vec::new(),
            users: Vec::new(),
            reads: HashMap::new(),
            typing: Vec::new(),
            unread: HashMap::new(),
            call: None,
            voice: Vec::new(),
            last_seq: 0,
            joined: false,
            history_complete: false,
            latency_ms: None,
            error_at: 0,
            rooms_version: 0,
            transport: if addr.starts_with("ws://") || addr.starts_with("wss://") {
                "WebSocket"
            } else {
                "TCP"
            },
            addr: addr.to_string(),
        }
    }

    pub fn current_room(&self) -> Option<&Room> {
        self.current
            .and_then(|id| self.rooms.iter().find(|r| r.id == id))
    }

    pub fn room_name(&self, id: u64) -> &str {
        self.rooms
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.as_str())
            .unwrap_or("комната")
    }

    /// Имена тех, кто печатает, кроме меня. Просроченные записи отсеиваются.
    pub fn typing_names(&self) -> Vec<&str> {
        self.typing
            .iter()
            .filter(|t| t.since.elapsed() < Duration::from_secs(4))
            .filter(|t| Some(t.id) != self.me)
            .map(|t| t.name.as_str())
            .collect()
    }

    pub fn unread_total(&self) -> u32 {
        self.unread.values().sum()
    }

    /// До какого сообщения собеседник дочитал.
    pub fn peer_read(&self, room: u64) -> u64 {
        self.reads.get(&room).copied().unwrap_or(0)
    }

    pub fn peer_of(&self, room: u64) -> Option<&User> {
        let peer = self.rooms.iter().find(|r| r.id == room)?.peer_id;
        self.users.iter().find(|u| u.id == peer)
    }
}

/// Настройки подключения.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub addr: String,
    pub name: String,
    pub password: String,
    pub invite: String,
    pub kind: ClientKind,
}

impl ClientConfig {
    pub fn new(addr: impl Into<String>, name: impl Into<String>, password: impl Into<String>) -> Self {
        ClientConfig {
            addr: addr.into(),
            name: name.into(),
            password: password.into(),
            invite: String::new(),
            kind: ClientKind::Desktop,
        }
    }
}

#[derive(Debug)]
enum Cmd {
    Send { room: u64, text: String },
    SendVoice { room: u64, data: Vec<u8> },
    SendSticker { room: u64, sticker: u8 },
    Join(u64),
    Leave(u64),
    Typing { room: u64, on: bool },
    OpenDirect { user: u64 },
    Read { room: u64, seq: u64 },
    SetProfile { handle: String, about: String, presence: u8 },
    CallInvite { to: u64 },
    CallAccept { to: u64 },
    CallDecline { to: u64 },
    CallEnd { to: u64 },
    Voice { to: u64, data: Vec<u8> },
    History { room: u64, before: u64 },
    Shutdown,
}

/// Клиент. Живёт, пока жива программа; сеть крутится в фоне.
pub struct Client {
    state: Arc<Mutex<State>>,
    tx: mpsc::Sender<Cmd>,
    stop: Arc<AtomicBool>,
    read_sent: Arc<Mutex<HashMap<u64, u64>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &self.state.lock().map(|s| s.status).ok())
            .finish()
    }
}

impl Client {
    /// Запускает подключение и сразу возвращает клиент.
    pub fn connect(mut cfg: ClientConfig) -> Client {
        cfg.addr = normalize_addr(&cfg.addr);
        let state = Arc::new(Mutex::new(State::new(&cfg.name, &cfg.addr)));
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let state2 = state.clone();
        let stop2 = stop.clone();
        let handle = thread::spawn(move || supervisor(cfg, state2, rx, stop2));
        Client {
            state,
            tx,
            stop,
            read_sent: Arc::new(Mutex::new(HashMap::new())),
            handle: Some(handle),
        }
    }

    pub fn snapshot(&self) -> State {
        self.lock().clone()
    }

    /// Читает состояние под замком — без клонирования истории.
    pub fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn send_voice_message(&self, data: Vec<u8>) {
        let room = self.lock().current;
        if let Some(room) = room {
            let _ = self.tx.send(Cmd::SendVoice { room, data });
        }
    }

    pub fn send_message(&self, text: &str) {
        let text = krug_core::sanitize_text(text);
        if text.is_empty() {
            return;
        }
        let room = self.lock().current;
        if let Some(room) = room {
            let _ = self.tx.send(Cmd::Send { room, text });
        }
    }

    /// Отправить стикер (1..=6) в текущий чат.
    pub fn send_sticker(&self, sticker: u8) {
        if !(1..=MAX_STICKER).contains(&sticker) {
            return;
        }
        let room = self.lock().current;
        if let Some(room) = room {
            let _ = self.tx.send(Cmd::SendSticker { room, sticker });
        }
    }

    /// Переключение комнаты: сразу чистим экран, сервер пришлёт историю.
    pub fn join(&self, room: u64) {
        {
            let mut state = self.lock();
            if state.current == Some(room) && state.joined {
                return;
            }
            state.current = Some(room);
            state.messages.clear();
            state.members.clear();
            state.typing.clear();
            state.joined = false;
            state.last_seq = 0;
            state.unread.remove(&room);
        }
        let _ = self.tx.send(Cmd::Join(room));
    }

    pub fn leave(&self, room: u64) {
        let _ = self.tx.send(Cmd::Leave(room));
    }

    /// Открыть личный чат с человеком.
    pub fn open_direct(&self, user: u64) {
        let _ = self.tx.send(Cmd::OpenDirect { user });
    }

    pub fn call(&self, user: u64) {
        {
            let mut state = self.lock();
            if state.call.is_some() {
                return;
            }
            let Some(peer) = state.users.iter().find(|u| u.id == user).cloned() else {
                return;
            };
            state.call = Some(CallState {
                peer,
                incoming: false,
                active: false,
                started_ms: 0,
                muted: false,
            });
        }
        let _ = self.tx.send(Cmd::CallInvite { to: user });
    }

    pub fn accept_call(&self) {
        let mut state = self.lock();
        let Some(call) = &mut state.call else {
            return;
        };
        let peer = call.peer.id;
        call.active = true;
        call.incoming = false;
        call.started_ms = now_ms();
        drop(state);
        let _ = self.tx.send(Cmd::CallAccept { to: peer });
    }

    pub fn decline_call(&self) {
        let peer = self.lock().call.as_ref().map(|call| call.peer.id);
        if let Some(peer) = peer {
            self.lock().call = None;
            let _ = self.tx.send(Cmd::CallDecline { to: peer });
        }
    }

    pub fn end_call(&self) {
        let peer = self.lock().call.as_ref().map(|call| call.peer.id);
        if let Some(peer) = peer {
            self.lock().call = None;
            let _ = self.tx.send(Cmd::CallEnd { to: peer });
        }
    }

    pub fn toggle_mute(&self) {
        if let Some(call) = &mut self.lock().call {
            call.muted = !call.muted;
        }
    }

    pub fn send_voice(&self, data: Vec<u8>) {
        let target = self
            .lock()
            .call
            .as_ref()
            .filter(|call| call.active)
            .map(|call| call.peer.id);
        if let Some(to) = target {
            let _ = self.tx.send(Cmd::Voice { to, data });
        }
    }

    pub fn take_voice(&self) -> Vec<(u64, Vec<u8>)> {
        std::mem::take(&mut self.lock().voice)
    }

    /// Сохранить ник, статус и «о себе».
    pub fn set_profile(&self, handle: &str, about: &str, presence: u8) {
        let _ = self.tx.send(Cmd::SetProfile {
            handle: handle.to_string(),
            about: about.to_string(),
            presence,
        });
    }

    /// Сообщить серверу, что текущий чат прочитан. Дубли не отправляются.
    pub fn mark_read(&self) {
        let (room, seq) = {
            let state = self.lock();
            match state.current {
                Some(room) if state.last_seq > 0 => (room, state.last_seq),
                _ => return,
            }
        };
        let mut sent = self.read_sent.lock().unwrap_or_else(|e| e.into_inner());
        if sent.get(&room).copied().unwrap_or(0) >= seq {
            return;
        }
        sent.insert(room, seq);
        drop(sent);
        let _ = self.tx.send(Cmd::Read { room, seq });
    }

    pub fn typing(&self, on: bool) {
        let room = self.lock().current;
        if let Some(room) = room {
            let _ = self.tx.send(Cmd::Typing { room, on });
        }
    }

    pub fn load_older(&self) {
        let (room, before) = {
            let state = self.lock();
            match (state.current, state.messages.first()) {
                (Some(room), Some(first)) if !state.history_complete => (room, first.seq),
                _ => return,
            }
        };
        let _ = self.tx.send(Cmd::History { room, before });
    }

    pub fn disconnect(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(handle) = self.handle.take() {
            // Не ждём: поток сам заметит stop в течение 100 мс.
            drop(handle);
        }
    }
}

// ---------------------------------------------------------------------------
// Транспорт

enum Poll {
    Frame(Vec<u8>),
    Idle,
    Closed,
}

enum Link {
    Tcp(TcpLink),
    Ws(WsLink),
}

struct TcpLink {
    stream: TcpStream,
    buf: Vec<u8>,
}

struct WsLink {
    ws: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
}

impl Link {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        match self {
            Link::Tcp(link) => write_frame(&mut link.stream, frame),
            Link::Ws(link) => {
                link.ws
                    .send(tungstenite::Message::Binary(frame.to_vec()))
                    .map_err(ws_error)?;
                link.ws.flush().map_err(ws_error)
            }
        }
    }

    fn poll(&mut self) -> io::Result<Poll> {
        match self {
            Link::Tcp(link) => link.poll(),
            Link::Ws(link) => link.poll(),
        }
    }

    fn close(&mut self) {
        match self {
            Link::Tcp(link) => {
                let _ = link.stream.shutdown(Shutdown::Both);
            }
            Link::Ws(link) => {
                let _ = link.ws.close(None);
                let _ = link.ws.flush();
                if let Some(tcp) = raw_tcp(&link.ws) {
                    let _ = tcp.shutdown(Shutdown::Both);
                }
            }
        }
    }
}

fn ws_error(e: tungstenite::Error) -> io::Error {
    match e {
        tungstenite::Error::Io(e) => e,
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}

fn raw_tcp(
    ws: &tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
) -> Option<&TcpStream> {
    match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => Some(s),
        tungstenite::stream::MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    }
}

impl TcpLink {
    fn take_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let n = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if n == 0 || n > MAX_FRAME {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "битый кадр"));
        }
        if self.buf.len() < 4 + n {
            return Ok(None);
        }
        let frame = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Ok(Some(frame))
    }

    fn poll(&mut self) -> io::Result<Poll> {
        loop {
            if let Some(frame) = self.take_frame()? {
                return Ok(Poll::Frame(frame));
            }
            let mut tmp = [0u8; 8192];
            match self.stream.read(&mut tmp) {
                Ok(0) => return Ok(Poll::Closed),
                Ok(n) => self.buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                    return Ok(Poll::Idle)
                }
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => return Ok(Poll::Closed),
                Err(e) => return Err(e),
            }
        }
    }
}

impl WsLink {
    fn poll(&mut self) -> io::Result<Poll> {
        loop {
            match self.ws.read() {
                Ok(tungstenite::Message::Binary(data)) => return Ok(Poll::Frame(data)),
                Ok(tungstenite::Message::Close(_)) => return Ok(Poll::Closed),
                Ok(tungstenite::Message::Ping(_)) => {
                    let _ = self.ws.flush();
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    return Ok(Poll::Idle)
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => return Ok(Poll::Closed),
                Err(e) => return Err(ws_error(e)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Фоновый поток

fn supervisor(
    cfg: ClientConfig,
    state: Arc<Mutex<State>>,
    rx: mpsc::Receiver<Cmd>,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = 1u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        set(&state, Status::Connecting, None);
        match open(&cfg, &state) {
            Ok(mut link) => {
                backoff = 1;
                let mut last_rx = Instant::now();
                let mut last_ping = Instant::now();
                let mut nonce = 0u32;
                loop {
                    if stop.load(Ordering::Relaxed) {
                        link.close();
                        return;
                    }
                    let mut shutdown = false;
                    loop {
                        match rx.try_recv() {
                            Ok(Cmd::Shutdown) => {
                                shutdown = true;
                                break;
                            }
                            Ok(cmd) => {
                                if !run_cmd(&mut link, cmd) {
                                    shutdown = true;
                                    break;
                                }
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                link.close();
                                return;
                            }
                        }
                    }
                    if shutdown {
                        link.close();
                        return;
                    }
                    match link.poll() {
                        Ok(Poll::Frame(bytes)) => {
                            last_rx = Instant::now();
                            if let Ok(op) = ServerOp::decode(&bytes) {
                                apply(&state, op);
                            }
                        }
                        Ok(Poll::Idle) => {}
                        Ok(Poll::Closed) => break,
                        Err(e) => {
                            set(&state, Status::Offline, Some(format!("сеть: {e}")));
                            break;
                        }
                    }
                    if last_ping.elapsed() >= PING_EVERY {
                        nonce = nonce.wrapping_add(1);
                        let _ = link.send(&ClientOp::Ping { nonce }.encode());
                        last_ping = Instant::now();
                    }
                    if last_rx.elapsed() > IDLE_TIMEOUT {
                        set(&state, Status::Offline, Some("сервер не отвечает".into()));
                        break;
                    }
                }
                link.close();
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                set(
                    &state,
                    Status::Offline,
                    Some("связь потеряна, переподключаюсь…".into()),
                );
            }
            Err(OpenError::Rejected(text)) => {
                set(&state, Status::Rejected, Some(text));
                return;
            }
            Err(OpenError::Network(text)) => {
                set(&state, Status::Offline, Some(text));
            }
        }

        // Пауза с проверкой остановки.
        let mut slept = 0u64;
        while slept < backoff * 1000 && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
            slept += 100;
        }
        backoff = (backoff * 2).min(10);
    }
}

fn run_cmd(link: &mut Link, cmd: Cmd) -> bool {
    let op = match cmd {
        Cmd::Shutdown => return false,
        Cmd::Send { room, text } => ClientOp::SendMessage { room, text, tag: 0 },
        Cmd::SendVoice { room, data } => ClientOp::SendVoice { room, data, tag: 0 },
        Cmd::SendSticker { room, sticker } => ClientOp::SendSticker {
            room,
            sticker,
            tag: 0,
        },
        Cmd::Join(room) => ClientOp::Join { room },
        Cmd::Leave(room) => ClientOp::Leave { room },
        Cmd::Typing { room, on } => ClientOp::Typing { room, on },
        Cmd::OpenDirect { user } => ClientOp::OpenDirect { user },
        Cmd::Read { room, seq } => ClientOp::Read { room, seq },
        Cmd::SetProfile {
            handle,
            about,
            presence,
        } => ClientOp::SetProfile {
            handle,
            about,
            presence,
        },
        Cmd::CallInvite { to } => ClientOp::CallInvite { to },
        Cmd::CallAccept { to } => ClientOp::CallAccept { to },
        Cmd::CallDecline { to } => ClientOp::CallDecline { to },
        Cmd::CallEnd { to } => ClientOp::CallEnd { to },
        Cmd::Voice { to, data } => ClientOp::Voice { to, data },
        Cmd::History { room, before } => ClientOp::History {
            room,
            before,
            limit: 50,
        },
    };
    if link.send(&op.encode()).is_err() {
        return false;
    }
    true
}

enum OpenError {
    Rejected(String),
    Network(String),
}

fn open(cfg: &ClientConfig, state: &Arc<Mutex<State>>) -> Result<Link, OpenError> {
    let mut link = if cfg.addr.starts_with("ws://") || cfg.addr.starts_with("wss://") {
        open_ws(cfg)?
    } else {
        open_tcp(cfg)?
    };

    let login = ClientOp::Login {
        version: PROTOCOL_VERSION,
        name: cfg.name.clone(),
        password: cfg.password.clone(),
        invite: cfg.invite.clone(),
        client: cfg.kind,
    };
    link.send(&login.encode())
        .map_err(|e| OpenError::Network(format!("не удалось отправить вход: {e}")))?;

    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let welcome = loop {
        if Instant::now() > deadline {
            link.close();
            return Err(OpenError::Network("сервер не ответил на вход".into()));
        }
        match link.poll() {
            Ok(Poll::Frame(bytes)) => match ServerOp::decode(&bytes) {
                Ok(ServerOp::Welcome { .. }) => break bytes,
                Ok(ServerOp::Error { text, .. }) => {
                    link.close();
                    return Err(OpenError::Rejected(text));
                }
                Ok(_) => continue,
                Err(_) => {
                    link.close();
                    return Err(OpenError::Network("сервер прислал мусор".into()));
                }
            },
            Ok(Poll::Idle) => {}
            Ok(Poll::Closed) => {
                link.close();
                return Err(OpenError::Network("сервер закрыл соединение".into()));
            }
            Err(e) => {
                link.close();
                return Err(OpenError::Network(format!("сеть: {e}")));
            }
        }
    };
    if let Ok(op) = ServerOp::decode(&welcome) {
        apply(state, op);
    }

    // Заходим в прежнюю комнату (или в первую попавшуюся).
    let room = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.current.or_else(|| guard.rooms.first().map(|r| r.id))
    };
    if let Some(room) = room {
        let _ = link.send(&ClientOp::Join { room }.encode());
    }
    Ok(link)
}

fn open_tcp(cfg: &ClientConfig) -> Result<Link, OpenError> {
    let addrs: Vec<_> = cfg
        .addr
        .to_socket_addrs()
        .map_err(|e| OpenError::Network(format!("адрес {} не разобрать: {e}", cfg.addr)))?
        .collect();
    let addr = addrs
        .first()
        .copied()
        .ok_or_else(|| OpenError::Network("пустой адрес сервера".into()))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| OpenError::Network(format!("сервер недоступен: {e}")))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(POLL));
    Ok(Link::Tcp(TcpLink {
        stream,
        buf: Vec::new(),
    }))
}

fn open_ws(cfg: &ClientConfig) -> Result<Link, OpenError> {
    let (ws, _response) = tungstenite::connect(cfg.addr.as_str())
        .map_err(|e| OpenError::Network(format!("WebSocket недоступен: {e}")))?;
    if let Some(tcp) = raw_tcp(&ws) {
        let _ = tcp.set_nodelay(true);
        let _ = tcp.set_read_timeout(Some(POLL));
    }
    Ok(Link::Ws(WsLink { ws }))
}

fn set(state: &Arc<Mutex<State>>, status: Status, error: Option<String>) {
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    guard.status = status;
    guard.error = error;
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn apply(state: &Arc<Mutex<State>>, op: ServerOp) {
    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
    match op {
        ServerOp::Welcome {
            user_id,
            server_name,
            motd,
            rooms,
            online,
            users,
        } => {
            s.me = Some(user_id);
            s.server_name = server_name;
            s.motd = motd;
            s.rooms = rooms;
            s.users = users;
            s.online = online;
            s.status = Status::Online;
            s.error = None;
            s.error_at = 0;
            s.rooms_version += 1;
            if s.current.is_none() {
                s.current = s.rooms.first().map(|r| r.id);
            }
        }
        ServerOp::Error { code, text } => {
            s.error = Some(text);
            s.error_at = now_ms();
            if let Some(code) = ErrorCode::from_u8(code) {
                if matches!(
                    code,
                    ErrorCode::BadInvite
                        | ErrorCode::BadPassword
                        | ErrorCode::BadName
                        | ErrorCode::BadVersion
                        | ErrorCode::ServerFull
                ) {
                    s.status = Status::Rejected;
                }
            }
        }
        ServerOp::Rooms { rooms } => {
            s.rooms = rooms;
            s.rooms_version += 1;
            let current_alive = s
                .current
                .map(|id| s.rooms.iter().any(|r| r.id == id))
                .unwrap_or(false);
            if !current_alive {
                s.current = None;
                s.joined = false;
            }
        }
        ServerOp::Joined {
            room,
            members,
            messages,
            reads,
        } => {
            s.current = Some(room.id);
            if let Some(existing) = s.rooms.iter_mut().find(|r| r.id == room.id) {
                *existing = room.clone();
            } else {
                s.rooms.push(room.clone());
                s.rooms_version += 1;
            }
            s.members = members;
            for read in reads {
                if Some(read.user) != s.me {
                    s.reads.insert(room.id, read.seq.max(0));
                }
            }
            s.last_seq = messages.last().map(|m| m.seq).unwrap_or(0);
            s.messages = messages;
            s.joined = true;
            s.history_complete = false;
            s.typing.clear();
            s.unread.remove(&room.id);
        }
        ServerOp::Message { msg, .. } => {
            if Some(msg.room) == s.current {
                if msg.seq > s.last_seq {
                    s.last_seq = msg.seq;
                    s.messages.push(msg.clone());
                    if s.messages.len() > 400 {
                        let cut = s.messages.len() - 400;
                        s.messages.drain(..cut);
                    }
                }
                s.typing.retain(|t| t.id != msg.author_id);
            } else {
                *s.unread.entry(msg.room).or_insert(0) += 1;
            }
        }
        ServerOp::Presence { room, members } => {
            if Some(room) == s.current {
                s.members = members;
            }
        }
        ServerOp::Typing {
            room,
            user_id,
            name,
            on,
        } => {
            if Some(room) == s.current {
                s.typing.retain(|t| t.id != user_id);
                if on {
                    s.typing.push(TypingUser {
                        id: user_id,
                        name,
                        since: Instant::now(),
                    });
                }
            }
        }
        ServerOp::History {
            room,
            messages,
            complete,
        } => {
            if Some(room) == s.current {
                let existing: Vec<u64> = s.messages.iter().map(|m| m.seq).collect();
                let mut older: Vec<Message> = messages
                    .into_iter()
                    .filter(|m| !existing.contains(&m.seq))
                    .collect();
                older.append(&mut s.messages);
                s.messages = older;
                s.history_complete = complete;
            }
        }
        ServerOp::Users { users } => {
            s.users = users;
        }
        ServerOp::CallInvite { from } => {
            if s.call.is_none() {
                s.call = Some(CallState {
                    peer: from,
                    incoming: true,
                    active: false,
                    started_ms: 0,
                    muted: false,
                });
            }
        }
        ServerOp::CallAccept { from } => {
            if let Some(call) = &mut s.call {
                if call.peer.id == from.id {
                    call.active = true;
                    call.incoming = false;
                    call.started_ms = now_ms();
                }
            }
        }
        ServerOp::CallDecline { from } => {
            if s.call.as_ref().map(|c| c.peer.id) == Some(from.id) {
                s.call = None;
                s.error = Some("Звонок отклонён".into());
                s.error_at = now_ms();
            }
        }
        ServerOp::CallEnd { from } => {
            if s.call.as_ref().map(|c| c.peer.id) == Some(from.id) {
                s.call = None;
                s.error = Some("Звонок завершён".into());
                s.error_at = now_ms();
            }
        }
        ServerOp::Voice { from, data } => {
            if s.call.as_ref().map(|c| c.active && c.peer.id == from).unwrap_or(false) {
                s.voice.push((from, data));
                if s.voice.len() > 40 {
                    let cut = s.voice.len() - 40;
                    s.voice.drain(..cut);
                }
            }
        }
        ServerOp::Profile { user } => {
            if let Some(existing) = s.users.iter_mut().find(|u| u.id == user.id) {
                *existing = user;
            } else {
                s.users.push(user);
            }
        }
        ServerOp::ReadReceipt { room, user, seq } => {
            if Some(user) != s.me {
                let cur = s.reads.get(&room).copied().unwrap_or(0);
                if seq > cur {
                    s.reads.insert(room, seq);
                }
            }
        }
        ServerOp::Pong { nonce } => {
            let now = now_ms() as u32;
            s.latency_ms = Some(now.wrapping_sub(nonce).min(9_999));
        }
    }
}

/// Принимает и `https://host`, и `http://host`, и адрес без схемы.
pub fn normalize_addr(addr: &str) -> String {
    let addr = addr.trim().trim_end_matches('/');
    if let Some(rest) = addr.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = addr.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        addr.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hosting_urls() {
        assert_eq!(
            normalize_addr("https://krug.onrender.com/"),
            "wss://krug.onrender.com"
        );
        assert_eq!(
            normalize_addr("http://127.0.0.1:8080"),
            "ws://127.0.0.1:8080"
        );
        assert_eq!(
            normalize_addr(" 127.0.0.1:7777 "),
            "127.0.0.1:7777"
        );
    }

    #[test]
    fn state_helpers() {
        let mut s = State::new("Аня", "127.0.0.1:7777");
        assert_eq!(s.transport, "TCP");
        s.typing.push(TypingUser {
            id: 2,
            name: "Боб".into(),
            since: Instant::now(),
        });
        assert_eq!(s.typing_names(), vec!["Боб"]);
        assert_eq!(s.rooms_version, 0);
    }

    #[test]
    fn ws_transport_detected() {
        let s = State::new("Аня", "wss://krug.onrender.com");
        assert_eq!(s.transport, "WebSocket");
    }
}
