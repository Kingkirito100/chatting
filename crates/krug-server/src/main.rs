//! Консольный запуск сервера «Круга».

use std::net::UdpSocket;
use std::path::PathBuf;

use krug_server::{Config, Server};

fn lan_ip() -> Option<String> {
    // Ничего не отправляем: connect на UDP лишь выбирает маршрут.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

fn main() {
    let mut cfg = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("[krug] у флага {arg} нет значения");
                std::process::exit(2);
            })
        };
        match arg.as_str() {
            "--addr" => cfg.addr = value(),
            "--ws-addr" => cfg.ws_addr = Some(value()),
            "--data" => cfg.data_dir = PathBuf::from(value()),
            "--name" => cfg.server_name = value(),
            "--invite" => cfg.invite = value(),
            "--motd" => cfg.motd = value(),
            "--max-users" => {
                cfg.max_users = value().parse().unwrap_or(64);
            }
            "-h" | "--help" => {
                println!(
                    "\
Круг — свой мессенджер для друзей.

  krug-server [флаги]

  --addr <ip:порт>    где слушать TCP (по умолчанию 0.0.0.0:7777)
  --ws-addr <ip:порт> включить WebSocket для хостингов (Render, Koyeb)
  --data <папка>      где хранить историю (по умолчанию ./data)
  --name <название>   имя сервера на экране входа
  --invite <код>      код приглашения (пусто — открытый сервер)
  --motd <текст>      приветственная строка
  --max-users <N>     сколько аккаунтов разрешено

Всё бесплатно и локально: ни базы, ни облака, ни подписки."
                );
                return;
            }
            other => {
                eprintln!("[krug] неизвестный флаг: {other} (см. --help)");
                std::process::exit(2);
            }
        }
    }

    // Бесплатные хостинги сами говорят, на каком порту слушать.
    if cfg.ws_addr.is_none() {
        if let Ok(port) = std::env::var("PORT") {
            if !port.is_empty() {
                cfg.ws_addr = Some(format!("0.0.0.0:{port}"));
            }
        }
    }

    println!("Pakost Chatting — сервер {}", env!("CARGO_PKG_VERSION"));
    println!("  имя:      {}", cfg.server_name);
    println!("  данные:   {}", cfg.data_dir.display());
    if let Some(ws) = &cfg.ws_addr {
        println!("  websocket: {ws}");
    }
    if cfg.invite.is_empty() {
        println!("  инвайт:   нет (сервер открыт, задайте --invite)");
    } else {
        println!("  инвайт:   {}", cfg.invite);
    }

    match Server::bind(cfg) {
        Ok(server) => {
            let addr = server
                .local_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "?".into());
            println!("  адрес:    {addr}");
            if let (Some(ip), Ok(a)) = (lan_ip(), server.local_addr()) {
                let private = ip.starts_with("10.")
                    || ip.starts_with("192.168.")
                    || ip.starts_with("127.")
                    || ip.starts_with("172.16.")
                    || ip.starts_with("172.17.")
                    || ip.starts_with("172.18.")
                    || ip.starts_with("172.19.")
                    || ip.starts_with("172.2")
                    || ip.starts_with("172.30.")
                    || ip.starts_with("172.31.");
                if private {
                    println!("  для друзей: {ip}:{}", a.port());
                }
            }
            println!("  старт… (Ctrl+C — остановить)");
            if let Err(e) = server.run() {
                eprintln!("[krug] сервер упал: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("[krug] не удалось запустить сервер: {e}");
            eprintln!("[krug] возможно, порт занят. Попробуйте --addr 0.0.0.0:7778");
            std::process::exit(1);
        }
    }
}
