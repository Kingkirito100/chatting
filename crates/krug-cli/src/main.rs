//! Консольный клиент «Круга» — для терминала, SSH и проверки сервера.
//!
//! Это же ядро, что и в окне: тот же протокол, тот же клиент. Консоль
//! показывает, что «Круг» умеет жить без единого пикселя графики.

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::{Local, TimeZone};
use krug_client::{Client, ClientConfig, Status};
use krug_core::ClientKind;

fn color(code: &str, text: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn dim(text: &str, c: bool) -> String {
    color("2", text, c)
}

fn accent(text: &str, c: bool) -> String {
    color("38;5;141", text, c)
}

fn time_of(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) as i64;
    match Local.timestamp_opt(secs, 0).single() {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "--:--".to_string(),
    }
}

struct Args {
    server: String,
    name: String,
    password: Option<String>,
    invite: String,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args {
        server: "127.0.0.1:7777".into(),
        name: "гость".into(),
        password: None,
        invite: String::new(),
    };
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" | "-s" => parsed.server = args.next().unwrap_or(parsed.server),
            "--name" | "-n" => parsed.name = args.next().unwrap_or(parsed.name),
            "--password" | "-p" => parsed.password = args.next(),
            "--invite" | "-i" => parsed.invite = args.next().unwrap_or_default(),
            "-h" | "--help" => {
                println!(
                    "\
Круг — консольный клиент.

  krug [сервер] [имя] [--invite КОД] [--password ПАРОЛЬ]

  -s, --server    адрес: 127.0.0.1:7777 или wss://krug.onrender.com
  -n, --name      ник
  -p, --password  пароль (спросим, если не задан)
  -i, --invite    код приглашения

Команды в чате:
  /rooms            список комнат
  /join <имя|id>    перейти в комнату
  /chat <имя>       открыть личный чат
  /who              кто в сети
  /older            подгрузить историю
  /quit             выйти"
                );
                std::process::exit(0);
            }
            other if !other.starts_with('-') => positional.push(other.to_string()),
            other => eprintln!("[krug] неизвестный флаг {other}"),
        }
    }
    if let Some(server) = positional.first() {
        parsed.server = server.clone();
    }
    if let Some(name) = positional.get(1) {
        parsed.name = name.clone();
    }
    parsed
}

fn ask_password(name: &str) -> String {
    print!("Пароль для {name}: ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
    line.trim().to_string()
}

fn main() {
    let args = parse_args();
    let tty = io::stdout().is_terminal();
    let password = args
        .password
        .clone()
        .unwrap_or_else(|| ask_password(&args.name));
    if password.len() < 4 {
        eprintln!("[krug] пароль должен быть не короче 4 символов");
        std::process::exit(2);
    }

    println!(
        "{} {}",
        accent("Круг", tty),
        dim(&format!("— {} как {}", args.server, args.name), tty)
    );
    println!("{}", dim("  /help — команды, /quit — выход", tty));

    let mut cfg = ClientConfig::new(args.server.clone(), args.name.clone(), password);
    cfg.invite = args.invite;
    cfg.kind = ClientKind::Terminal;
    let client = Client::connect(cfg);

    let (line_tx, line_rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut running = true;
    let mut last_status = Status::Connecting;
    let mut last_error: Option<String> = None;
    let mut last_room: Option<u64> = None;
    let mut last_seq = 0u64;

    while running {
        while let Ok(line) = line_rx.try_recv() {
            if !handle_line(&client, &line, tty) {
                running = false;
                break;
            }
        }
        if !running {
            break;
        }

        let state = client.snapshot();

        if state.current != last_room {
            last_room = state.current;
            last_seq = 0;
            if let Some(room) = state.current_room() {
                println!(
                    "\n{} {}",
                    accent(&format!("чат с {}", room.name), tty),
                    dim(&room.topic, tty)
                );
            }
        }

        if state.status != last_status {
            last_status = state.status;
            match state.status {
                Status::Online => println!(
                    "{} {}",
                    color("32", "●", tty),
                    dim(
                        &format!(
                            "подключено к «{}» ({}){}",
                            state.server_name,
                            state.transport,
                            state
                                .latency_ms
                                .map(|ms| format!(", {ms} мс"))
                                .unwrap_or_default()
                        ),
                        tty
                    )
                ),
                Status::Connecting => println!("{}", dim("… подключаюсь", tty)),
                Status::Offline => println!("{}", color("33", "○ нет связи, переподключаюсь", tty)),
                Status::Rejected => println!("{}", color("31", "✕ сервер отказал", tty)),
            }
        }

        if state.error != last_error {
            last_error = state.error.clone();
            if let Some(error) = &state.error {
                println!("{}", color("31", &format!("! {error}"), tty));
            }
        }

        let fresh: Vec<_> = state
            .messages
            .iter()
            .filter(|m| m.seq > last_seq)
            .cloned()
            .collect();
        for msg in fresh {
            last_seq = msg.seq;
            let mine = Some(msg.author_id) == state.me;
            let name = if mine {
                accent(&msg.author, tty)
            } else {
                color("36", &msg.author, tty)
            };
            let indent = " ".repeat(msg.author.chars().count().min(20));
            for (i, part) in msg.text.lines().enumerate() {
                if i == 0 {
                    println!("{} {} {}", dim(&time_of(msg.ts_ms), tty), name, part);
                } else {
                    println!("{} {} {}", dim("     ", tty), indent, part);
                }
            }
        }

        thread::sleep(Duration::from_millis(80));
    }

    client.disconnect();
    println!("{}", dim("пока.", tty));
}

/// Возвращает `false`, если пора выходить.
fn handle_line(client: &Client, line: &str, tty: bool) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return true;
    }
    if !line.starts_with('/') {
        client.send_message(line);
        client.typing(false);
        return true;
    }
    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/quit" | "/exit" | "/q" => return false,
        "/help" | "/?" => {
            println!(
                "{}",
                dim(
                    "/rooms  /join <имя|id>  /chat <имя>  /who  /older  /clear  /quit",
                    tty
                )
            );
        }
        "/rooms" => {
            let state = client.snapshot();
            for room in &state.rooms {
                let mark = if Some(room.id) == state.current { "→" } else { " " };
                let unread = state.unread.get(&room.id).copied().unwrap_or(0);
                let unread = if unread > 0 {
                    format!(" ({unread} новых)")
                } else {
                    String::new()
                };
                println!(
                    "{} #{}  {}{}",
                    mark,
                    room.name,
                    dim(&format!("{} чел.", room.online), tty),
                    unread
                );
            }
        }
        "/join" => {
            let state = client.snapshot();
            let target = state.rooms.iter().find(|r| {
                r.name.eq_ignore_ascii_case(arg) || r.id.to_string() == arg
            });
            match target {
                Some(room) => client.join(room.id),
                None => {
                    if let Some(user) = state.users.iter().find(|u| u.name.eq_ignore_ascii_case(arg)) {
                        client.open_direct(user.id);
                    } else {
                        println!("{}", color("31", "такого чата нет — /chat <имя>", tty));
                    }
                }
            }
        }
        "/chat" => {
            let state = client.snapshot();
            match state.users.iter().find(|u| u.name.eq_ignore_ascii_case(arg)) {
                Some(user) => client.open_direct(user.id),
                None => println!("{}", color("31", "такого человека нет — /who", tty)),
            }
        }
        "/who" => {
            let state = client.snapshot();
            let mut names: Vec<String> = state
                .users
                .iter()
                .filter(|u| u.status == 0 && Some(u.id) != state.me)
                .map(|u| u.name.clone())
                .collect();
            if names.is_empty() {
                println!("{}", dim("никого, кроме тебя", tty));
            } else {
                names.sort();
                println!("{} {}", dim("в сети:", tty), names.join(", "));
            }
        }
        "/older" => client.load_older(),
        "/clear" => print!("\x1b[2J\x1b[H"),
        other => println!("{}", color("31", &format!("не знаю команду {other}"), tty)),
    }
    true
}
