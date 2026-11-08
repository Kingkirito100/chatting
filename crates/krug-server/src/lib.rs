//! Сервер «Круга»: TCP для своего железа и WebSocket для бесплатных
//! хостингов, где наружу смотрит только HTTP(S).
//!
//! Один бинарник, без базы и облачных сервисов.

pub mod hub;
pub mod store;

use std::io::{self, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use krug_core::frame::{read_frame, write_frame};
use krug_core::{ClientOp, MAX_FRAME};
pub use hub::{Action, Shared};

/// Настройки сервера.
#[derive(Debug, Clone)]
pub struct Config {
    pub addr: String,
    pub data_dir: PathBuf,
    pub server_name: String,
    /// Пустая строка — сервер открыт для всех.
    pub invite: String,
    pub motd: String,
    pub max_users: usize,
    /// Адрес WebSocket-слушателя, если нужен (Render/Koyeb/Cloudflare).
    pub ws_addr: Option<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            addr: "0.0.0.0:7777".to_string(),
            data_dir: PathBuf::from("data"),
            server_name: "Pakost Chatting".to_string(),
            invite: String::new(),
            motd: "Свои люди, свои игры.".to_string(),
            max_users: 64,
            ws_addr: None,
        }
    }
}

pub struct Server {
    listener: TcpListener,
    ws_listener: Option<TcpListener>,
    shared: Arc<Shared>,
    next_conn: Arc<AtomicU64>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("addr", &self.listener.local_addr().ok())
            .field("ws", &self.ws_listener.is_some())
            .finish()
    }
}

impl Server {
    /// Готовит хранилище и слушающие сокеты.
    pub fn bind(cfg: Config) -> io::Result<Server> {
        let shared = Arc::new(Shared::new(cfg.clone())?);
        let listener = TcpListener::bind(&cfg.addr)?;
        listener.set_nonblocking(true)?;
        let ws_listener = match &cfg.ws_addr {
            Some(addr) => {
                let l = TcpListener::bind(addr)?;
                l.set_nonblocking(true)?;
                Some(l)
            }
            None => None,
        };
        Ok(Server {
            listener,
            ws_listener,
            shared,
            next_conn: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn ws_local_addr(&self) -> Option<SocketAddr> {
        self.ws_listener
            .as_ref()
            .and_then(|listener| listener.local_addr().ok())
    }

    pub fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// Блокирующий цикл приёма соединений.
    pub fn run(mut self) -> io::Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        self.start_ws(stop.clone());
        self.serve(stop);
        Ok(())
    }

    /// Запуск в фоновом потоке — удобно для тестов и встраивания.
    pub fn spawn(mut self) -> io::Result<ServerHandle> {
        let addr = self.listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        self.start_ws(stop.clone());
        let stop2 = stop.clone();
        let join = thread::spawn(move || self.serve(stop2));
        Ok(ServerHandle {
            addr,
            stop,
            join: Some(join),
        })
    }

    fn start_ws(&mut self, stop: Arc<AtomicBool>) {
        let Some(listener) = self.ws_listener.take() else {
            return;
        };
        let shared = self.shared.clone();
        let next_conn = self.next_conn.clone();
        thread::spawn(move || serve_ws(listener, shared, next_conn, stop));
    }

    fn serve(&mut self, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
                    self.spawn_conn(id, stream, peer);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(15));
                }
                Err(e) => {
                    eprintln!("[krug] ошибка accept: {e}");
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn spawn_conn(&self, id: u64, stream: TcpStream, peer: SocketAddr) {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_nonblocking(false);
        let write_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[krug] не смог разделить сокет: {e}");
                return;
            }
        };
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut ws = write_stream;
            while let Ok(frame) = rx.recv() {
                if write_frame(&mut ws, &frame).is_err() {
                    break;
                }
            }
            let _ = ws.shutdown(Shutdown::Both);
        });
        self.shared.attach(id, tx);
        let shared = self.shared.clone();
        thread::spawn(move || serve_conn(shared, stream, id, peer));
    }
}

fn serve_conn(shared: Arc<Shared>, mut stream: TcpStream, id: u64, peer: SocketAddr) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(90)));
    loop {
        let payload = match read_frame(&mut stream, MAX_FRAME) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(e) => {
                if e.kind() != io::ErrorKind::TimedOut && e.kind() != io::ErrorKind::WouldBlock {
                    eprintln!("[krug] чтение от {peer}: {e}");
                }
                break;
            }
        };
        match handle_frame(&shared, id, &payload) {
            Action::Continue => {}
            Action::Close => break,
        }
    }
    shared.detach(id);
}

fn handle_frame(shared: &Shared, id: u64, payload: &[u8]) -> Action {
    match ClientOp::decode(payload) {
        Ok(op) => shared.handle(id, op),
        Err(e) => {
            shared.bad_request(id, &e.to_string());
            Action::Close
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket

fn serve_ws(
    listener: TcpListener,
    shared: Arc<Shared>,
    next_conn: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_nodelay(true);
                let id = next_conn.fetch_add(1, Ordering::Relaxed);
                let shared = shared.clone();
                thread::spawn(move || serve_ws_conn(shared, stream, id, peer));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                eprintln!("[krug] ошибка accept ws: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn serve_ws_conn(shared: Arc<Shared>, mut stream: TcpStream, id: u64, peer: SocketAddr) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let head = match read_request_head(&mut stream) {
        Ok(v) => v,
        Err(_) => return,
    };
    let text = String::from_utf8_lossy(&head);
    let first = text.lines().next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("/");
    let websocket = is_websocket(&text);
    eprintln!("[krug] http {first} upgrade={websocket}");
    if !websocket {
        let _ = health_response(&mut stream, path);
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
    let mut ws = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[krug] ws-рукопожатие от {peer}: {e}");
            return;
        }
    };
    let writer_stream = match ws.get_ref().try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    shared.attach(id, tx);
    let writer = thread::spawn(move || {
        let mut stream = writer_stream;
        while let Ok(frame) = rx.recv() {
            if write_ws_frame(&mut stream, &frame).is_err() {
                break;
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
    });

    // Читаем без спешки: исходящие кадры шлёт отдельный поток, задержки нет.
    let _ = ws
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(500)));
    loop {
        match ws.read() {
            Ok(tungstenite::Message::Binary(data)) => match handle_frame(&shared, id, &data) {
                Action::Continue => {}
                Action::Close => break,
            },
            Ok(tungstenite::Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {}
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                break
            }
            Err(_) => break,
        }
    }
    shared.detach(id);
    drop(ws);
    let _ = writer.join();
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut buf = [0u8; 8192];
    for _ in 0..200 {
        let n = stream.peek(&mut buf)?;
        if n == 0 {
            return Ok(Vec::new());
        }
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf[..n].to_vec());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let n = stream.peek(&mut buf)?;
    Ok(buf[..n].to_vec())
}

fn is_websocket(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let upgrade = lower
        .lines()
        .any(|l| l.starts_with("upgrade:") && l.contains("websocket"));
    let connection = lower
        .lines()
        .any(|l| l.starts_with("connection:") && l.contains("upgrade"));
    upgrade && connection
}

fn health_response(stream: &mut TcpStream, path: &str) -> io::Result<()> {
    if path == "/health" {
        let body = "ok";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())?;
        return stream.flush();
    }
    if path == "/diag" {
        let body = format!(
            "{{\"app\":\"Pakost Chatting\",\"version\":\"{}\",\"ws_listener\":true}}",
            env!("CARGO_PKG_VERSION")
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes())?;
        return stream.flush();
    }
    // На обычный GET отдаём мобильный клиент: один адрес и для страницы, и для WS.
    let body = include_str!("../../../mobile/index.html");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Кадр WebSocket от сервера клиенту: без маски, бинарный.
fn write_ws_frame<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    let mut header = Vec::with_capacity(10);
    header.push(0x82); // FIN + binary
    let len = payload.len();
    if len < 126 {
        header.push(len as u8);
    } else if len <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Дескриптор фонового сервера (для тестов).
#[derive(Debug)]
pub struct ServerHandle {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
