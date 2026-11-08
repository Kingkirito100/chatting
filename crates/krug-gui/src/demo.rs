//! Демонстрационные данные для скриншотов и режима `--demo`.
//!
//! Ровно те же структуры, что приходят с сервера, — значит, скриншот
//! проверяет настоящую вёрстку, а не отдельный «макетный» код.

use std::collections::HashMap;

use krug_client::{State, Status};
use krug_core::model::{Message, Room, User};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn msg(room: u64, seq: u64, minutes_ago: u64, id: u64, author: &str, text: &str) -> Message {
    Message {
        room,
        seq,
        ts_ms: now_ms().saturating_sub(minutes_ago * 60_000),
        author_id: id,
        author: author.to_string(),
        text: text.to_string(),
        voice: false,
        sticker: 0,
        audio: Vec::new(),
    }
}

fn person(id: u64, name: &str, handle: &str, presence: u8) -> User {
    User {
        id,
        name: name.into(),
        handle: handle.into(),
        about: if name == "Боб" { "играю вечером".into() } else { String::new() },
        status: 0,
        presence,
    }
}

pub fn state() -> State {
    let mut state = State::new("Аня", "wss://krug.onrender.com");
    state.status = Status::Online;
    state.server_name = "Pakost Chatting".into();
    state.motd = "Свои люди, свои игры.".into();
    state.me = Some(1);
    state.online = 5;
    state.joined = true;
    state.latency_ms = Some(24);
    state.users = vec![
        person(1, "Аня", "anya", 0),
        person(2, "Боб", "bob", 0),
        person(3, "Костя", "kostya", 1),
        person(5, "Лиза", "liza", 0),
        person(8, "Марк", "mark", 3),
    ];
    state.rooms = vec![
        Room { id: 1, name: "Боб".into(), topic: String::new(), kind: krug_core::model::ROOM_DM, online: 1, peer_id: 2 },
        Room { id: 2, name: "Костя".into(), topic: String::new(), kind: krug_core::model::ROOM_DM, online: 1, peer_id: 3 },
        Room { id: 3, name: "Лиза".into(), topic: String::new(), kind: krug_core::model::ROOM_DM, online: 1, peer_id: 5 },
        Room { id: 4, name: "Марк".into(), topic: String::new(), kind: krug_core::model::ROOM_DM, online: 0, peer_id: 8 },
    ];
    state.current = Some(1);
    state.members = vec![
        person(1, "Аня", "anya", 0),
        person(2, "Боб", "bob", 0),
    ];
    state.messages = vec![
        msg(1, 1, 47, 2, "Боб", "кто вечером в катку?"),
        msg(1, 2, 45, 1, "Аня", "я в деле, во сколько?"),
        msg(1, 3, 45, 2, "Боб", "давай в восемь"),
        msg(1, 4, 41, 1, "Аня", "у меня как раз микрофон починился, так что я тоже"),
        msg(1, 5, 38, 2, "Боб", "го сюда. Тут история не теряется, и никто не выкинет нас на другой сервер."),
        msg(1, 6, 36, 1, "Аня", "а сервер где живёт?"),
        msg(1, 7, 35, 2, "Боб", "на бесплатном хостинге. Поднимать ничего не надо, ссылку скину в личку."),
        msg(1, 8, 31, 1, "Аня", "звучит лучше дискорда. Только не забудь про уведомления"),
        msg(1, 9, 28, 2, "Боб", "а можно я приведу Диму?"),
    ];
    state.typing = vec![krug_client::TypingUser {
        id: 2,
        name: "Боб".into(),
        since: std::time::Instant::now(),
    }];
    state.reads.insert(1, 7);
    state.unread = HashMap::from([(2, 3), (3, 12)]);
    state.last_seq = 9;
    state
}
