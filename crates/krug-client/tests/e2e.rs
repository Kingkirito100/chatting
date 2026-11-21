//! Сквозные тесты: личные чаты, прочтения, история и WebSocket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use krug_client::{Client, ClientConfig, Status};
use krug_core::ClientKind;
use krug_server::store::{Store, UserRecord};
use krug_server::{Config, Server};

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("krug-e2e-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn config(addr: &str, dir: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.addr = addr.to_string();
    cfg.data_dir = dir.to_path_buf();
    cfg.invite = "свои".into();
    cfg.max_users = 8;
    cfg
}

fn wait<F: FnMut() -> bool>(mut check: F, what: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8) {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!("не дождались: {what}");
}

fn connect(addr: &str, name: &str) -> Client {
    let mut cfg = ClientConfig::new(addr, name, "пароль123");
    cfg.invite = "свои".into();
    cfg.kind = ClientKind::Terminal;
    Client::connect(cfg)
}

fn online(client: &Client) -> bool {
    client.read(|s| s.status == Status::Online)
}

fn user_id(client: &Client, name: &str) -> u64 {
    client.read(|s| {
        s.users
            .iter()
            .find(|u| u.name == name)
            .map(|u| u.id)
            .unwrap_or(0)
    })
}

#[test]
fn direct_chat_reads_and_history_survives_restart() {
    let dir = temp_dir("dm");
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let mut handle = server.spawn().unwrap();

    let anya = connect(&addr, "Аня");
    wait(|| online(&anya), "Аня в сети");
    let bob = connect(&addr, "Боб");
    wait(|| online(&bob), "Боб в сети");

    // Никаких каналов: новых чатов при входе нет.
    assert!(anya.read(|s| s.rooms.is_empty()));

    wait(
        || {
            anya.read(|s| s.users.iter().any(|u| u.name == "Боб"))
                && bob.read(|s| s.users.iter().any(|u| u.name == "Аня"))
        },
        "списки людей пришли",
    );
    // Ник и статус синхронизируются на всех клиентов.
    anya.set_profile_avatar("anya_test", "на связи", 1, 8);
    wait(
        || {
            bob.read(|s| {
                s.users.iter().any(|u| {
                    u.name == "Аня"
                        && u.handle == "anya_test"
                        && u.about == "на связи"
                        && u.presence == 1
                        && u.avatar == 8
                })
            })
        },
        "профиль обновился у Боба",
    );

    let bob_id = user_id(&anya, "Боб");
    anya.open_direct(bob_id);
    wait(
        || {
            anya.read(|s| {
                s.joined
                    && s.current_room()
                        .map(|r| r.peer_id == bob_id)
                        .unwrap_or(false)
            })
        },
        "Аня открыла личный чат",
    );
    let room = anya.read(|s| s.current.unwrap());

    wait(
        || bob.read(|s| s.rooms.iter().any(|r| r.id == room)),
        "чат появился у Боба",
    );
    bob.join(room);
    wait(|| bob.read(|s| s.joined), "Боб открыл чат");

    anya.send_message("привет из теста");
    wait(
        || bob.read(|s| s.messages.iter().any(|m| m.text == "привет из теста")),
        "Боб получил сообщение",
    );

    // Метка о прочтении: Боб прочитал — Аня видит ✓✓.
    bob.mark_read();
    wait(
        || anya.read(|s| s.peer_read(room) >= 1),
        "Аня получила отметку о прочтении",
    );

    anya.send_voice_message(vec![1, 2, 3, 4, 5, 6]);
    wait(
        || bob.read(|s| s.messages.iter().any(|m| m.voice && !m.audio.is_empty())),
        "голосовое сообщение дошло",
    );

    anya.send_sticker(2);
    wait(
        || {
            bob.read(|s| {
                s.messages
                    .iter()
                    .any(|m| m.sticker == 2 && m.text.is_empty() && !m.voice)
            })
        },
        "стикер дошёл",
    );

    bob.send_message("ответ Боба");
    wait(
        || anya.read(|s| s.messages.iter().any(|m| m.text == "ответ Боба")),
        "Аня получила ответ",
    );

    drop(anya);
    drop(bob);
    handle.stop();
    drop(handle);

    // Перезапуск на тех же данных: аккаунты, чат и история должны выжить.
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let _handle = server.spawn().unwrap();

    let anya = connect(&addr, "Аня");
    wait(|| online(&anya), "Аня снова в сети");
    anya.open_direct(bob_id);
    wait(
        || anya.read(|s| s.joined),
        "Аня открыла чат после перезапуска",
    );
    let messages = anya.read(|s| {
        s.messages
            .iter()
            .map(|m| m.text.clone())
            .collect::<Vec<_>>()
    });
    assert!(
        messages.iter().any(|t| t == "привет из теста"),
        "история: {messages:?}"
    );
    assert!(
        messages.iter().any(|t| t == "ответ Боба"),
        "история: {messages:?}"
    );
    let stickers = anya.read(|s| s.messages.iter().map(|m| m.sticker).collect::<Vec<_>>());
    assert!(
        stickers.contains(&2),
        "стикер потерялся в истории: {stickers:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn websocket_transport_and_health_endpoint() {
    let dir = temp_dir("ws");
    let mut cfg = config("127.0.0.1:0", &dir);
    cfg.ws_addr = Some("127.0.0.1:0".into());
    let server = Server::bind(cfg).unwrap();
    let ws_addr = server.ws_local_addr().expect("ws-адрес");
    let tcp_addr = server.local_addr().unwrap().to_string();
    let _handle = server.spawn().unwrap();

    let mut http = TcpStream::connect(ws_addr).unwrap();
    http.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    http.write_all(b"GET / HTTP/1.1\r\nHost: krug\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    http.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "ответ: {response:?}");

    let url = format!("ws://{ws_addr}");
    let mut cfg = ClientConfig::new(url, "Аня", "пароль123");
    cfg.invite = "свои".into();
    cfg.kind = ClientKind::Terminal;
    let anya = Client::connect(cfg);
    wait(|| online(&anya), "вход через WebSocket");

    let bob = connect(&tcp_addr, "Боб");
    wait(|| online(&bob), "второй вход");
    wait(|| anya.read(|s| s.users.len() >= 2), "люди пришли");
    // Ник и статус синхронизируются на всех клиентов.
    anya.set_profile("anya_test", "на связи", 1);
    wait(
        || {
            bob.read(|s| {
                s.users.iter().any(|u| {
                    u.name == "Аня"
                        && u.handle == "anya_test"
                        && u.about == "на связи"
                        && u.presence == 1
                })
            })
        },
        "профиль обновился у Боба",
    );

    let bob_id = user_id(&anya, "Боб");
    anya.open_direct(bob_id);
    wait(|| anya.read(|s| s.joined), "чат открылся через WebSocket");
    let room = anya.read(|s| s.current.unwrap());
    bob.join(room);
    wait(|| bob.read(|s| s.joined), "Боб вошёл");

    anya.send_message("через вебсокет");
    wait(
        || bob.read(|s| s.messages.iter().any(|m| m.text == "через вебсокет")),
        "сообщение дошло по WebSocket",
    );
    anya.disconnect();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn call_signaling_and_voice_relay() {
    let dir = temp_dir("call");
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let _handle = server.spawn().unwrap();

    let anya = connect(&addr, "Аня");
    wait(|| online(&anya), "Аня в сети");
    let bob = connect(&addr, "Боб");
    wait(|| online(&bob), "Боб в сети");
    wait(|| anya.read(|s| s.users.len() >= 2), "люди пришли");
    let bob_id = user_id(&anya, "Боб");
    let anya_id = user_id(&bob, "Аня");

    anya.call(bob_id);
    wait(
        || bob.read(|s| s.call.as_ref().map(|c| c.incoming).unwrap_or(false)),
        "входящий звонок",
    );
    bob.accept_call();
    wait(
        || {
            anya.read(|s| s.call.as_ref().map(|c| c.active).unwrap_or(false))
                && bob.read(|s| s.call.as_ref().map(|c| c.active).unwrap_or(false))
        },
        "звонок принят",
    );

    bob.send_voice(vec![1, 2, 3, 4]);
    wait(
        || anya.read(|s| !s.voice.is_empty()),
        "голосовой кадр дошёл",
    );
    anya.send_voice(vec![9, 8, 7]);
    wait(|| bob.read(|s| !s.voice.is_empty()), "обратный кадр дошёл");

    anya.end_call();
    wait(|| bob.read(|s| s.call.is_none()), "звонок завершён у Боба");
    let _ = anya_id;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sticker_reaches_peer_in_direct_chat() {
    let dir = temp_dir("sticker");
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let _handle = server.spawn().unwrap();

    let anya = connect(&addr, "Аня");
    wait(|| online(&anya), "Аня в сети");
    let bob = connect(&addr, "Боб");
    wait(|| online(&bob), "Боб в сети");
    wait(|| anya.read(|s| s.users.len() >= 2), "люди пришли");

    let bob_id = user_id(&anya, "Боб");
    anya.open_direct(bob_id);
    wait(|| anya.read(|s| s.joined), "Аня открыла личный чат");
    let room = anya.read(|s| s.current.unwrap());
    bob.join(room);
    wait(|| bob.read(|s| s.joined), "Боб открыл чат");

    anya.send_sticker(2);
    wait(
        || bob.read(|s| s.messages.iter().any(|m| m.sticker == 2)),
        "Боб получил стикер",
    );
    let sticker = bob.read(|s| {
        s.messages
            .iter()
            .find(|m| m.sticker == 2)
            .cloned()
            .expect("стикер в переписке")
    });
    assert_eq!(sticker.sticker, 2);
    assert!(sticker.text.is_empty());
    assert!(!sticker.voice);
    assert_eq!(sticker.author_id, anya.read(|s| s.me.unwrap()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registration_works_without_invite() {
    let dir = temp_dir("invite");
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let _handle = server.spawn().unwrap();

    let mut cfg = ClientConfig::new(&addr, "Чужой", "пароль123");
    cfg.invite.clear();
    cfg.kind = ClientKind::Terminal;
    let client = Client::connect(cfg);
    wait(
        || client.read(|s| s.status == Status::Online),
        "регистрация без инвайта",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn first_account_is_admin_and_admin_actions_are_authorized_and_persisted() {
    let dir = temp_dir("admin");
    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let mut handle = server.spawn().unwrap();

    let admin = connect(&addr, "Владелец");
    wait(|| online(&admin), "первый аккаунт в сети");
    assert_eq!(
        admin.read(|s| s.role),
        1,
        "первая учётка должна стать администратором"
    );
    admin.set_profile_avatar("owner_avatar", "", 0, 8);
    wait(
        || {
            admin.read(|s| {
                s.users
                    .iter()
                    .any(|u| u.id == s.me.unwrap() && u.avatar == 8)
            })
        },
        "аватар профиля обновился",
    );

    let member = connect(&addr, "Участник");
    wait(|| online(&member), "обычный пользователь в сети");
    assert_eq!(member.read(|s| s.role), 0);

    member.request_admin_snapshot();
    wait(
        || member.read(|s| s.error.as_deref() == Some("Недостаточно прав администратора")),
        "сервер отклонил запрос обычного пользователя",
    );
    assert!(member.read(|s| s.admin_users.is_empty()));

    admin.request_admin_snapshot();
    wait(
        || admin.read(|s| s.admin_users.len() == 2),
        "администратор получил snapshot",
    );
    let member_id = user_id(&admin, "Участник");
    admin.admin_set_user(member_id, 1, false);
    wait(
        || {
            admin.read(|s| {
                s.admin_users
                    .iter()
                    .any(|u| u.id == member_id && u.role == 1)
            })
        },
        "роль пользователя обновилась",
    );
    wait(
        || member.read(|s| s.role == 1),
        "роль пришла в активную сессию",
    );

    admin.admin_set_user(member_id, 1, true);
    wait(
        || {
            admin.read(|s| {
                s.admin_users
                    .iter()
                    .any(|u| u.id == member_id && u.disabled && !u.online)
            })
        },
        "пользователь отключён и сессия закрыта",
    );

    let owner_id = admin.read(|s| s.me.unwrap());
    admin.admin_set_user(owner_id, 0, false);
    wait(
        || {
            admin.read(|s| {
                s.error.as_deref()
                    == Some("Нельзя снять права или отключить собственную учётную запись")
            })
        },
        "нельзя снять собственные права",
    );
    assert_eq!(admin.read(|s| s.role), 1);

    let blocked = connect(&addr, "Участник");
    wait(
        || blocked.read(|s| s.status == Status::Rejected),
        "отключённый пользователь не может войти повторно",
    );

    drop(member);
    drop(blocked);
    drop(admin);
    handle.stop();

    let restart_cfg = {
        let mut cfg = config(&addr, &dir);
        cfg.addr = addr.clone();
        cfg
    };
    let restarted = Server::bind(restart_cfg).unwrap();
    let mut restarted_handle = restarted.spawn().unwrap();
    let owner_again = connect(&addr, "Владелец");
    wait(
        || online(&owner_again),
        "администратор вошёл после рестарта",
    );
    assert_eq!(
        owner_again.read(|s| s.role),
        1,
        "роль должна сохраняться после рестарта"
    );
    assert!(owner_again.read(|s| s
        .users
        .iter()
        .any(|u| u.id == s.me.unwrap() && u.avatar == 8)));
    owner_again.request_admin_snapshot();
    wait(
        || owner_again.read(|s| s.admin_users.len() == 2),
        "состояние пользователей после рестарта",
    );
    assert!(owner_again.read(|s| s
        .admin_users
        .iter()
        .any(|u| u.id == member_id && u.role == 1 && u.disabled)));

    drop(owner_again);
    restarted_handle.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_profile_file_migrates_and_bootstraps_oldest_account() {
    let dir = temp_dir("admin-migration");
    let store = Store::open(&dir).unwrap();
    let salt = [7u8; 16];
    let hash = krug_core::sha256::hash_password(&salt, "пароль123");
    store
        .append_user(&UserRecord {
            id: 7,
            name: "Старый".into(),
            salt,
            hash,
        })
        .unwrap();
    std::fs::write(dir.join("profiles.tsv"), "7\told_handle\tlegacy about\t2\n").unwrap();

    let server = Server::bind(config("127.0.0.1:0", &dir)).unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let mut handle = server.spawn().unwrap();
    let mut cfg = ClientConfig::new(&addr, "Старый", "пароль123");
    cfg.kind = ClientKind::Terminal;
    let client = Client::connect(cfg);
    wait(
        || online(&client),
        "существующий аккаунт вошёл после миграции",
    );
    assert_eq!(client.read(|s| s.role), 1);
    let migrated = std::fs::read_to_string(dir.join("profiles.tsv")).unwrap();
    assert!(migrated
        .lines()
        .any(|line| line == "7\told_handle\tlegacy about\t2\t1\t0\t0"));

    drop(client);
    handle.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
