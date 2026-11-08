//! Операции протокола «Круга».
//!
//! Кадр — это один байт операции и поля. Поверх TCP/WebSocket кадр
//! обрамляется четырёхбайтовой длиной (см. `krug-core::frame`).

use alloc::string::String;
use alloc::vec::Vec;

use crate::model::{Message, ReadState, Room, User};
use crate::wire::{ProtoError, Reader, Result, Writer};
use crate::{MAX_BATCH, MAX_MEMBERS, MAX_ROOMS};

pub const OP_LOGIN: u8 = 0x01;
pub const OP_SEND: u8 = 0x02;
pub const OP_JOIN: u8 = 0x03;
pub const OP_LEAVE: u8 = 0x04;
pub const OP_HISTORY: u8 = 0x05;
pub const OP_TYPING: u8 = 0x06;
pub const OP_OPEN_DIRECT: u8 = 0x07;
pub const OP_READ: u8 = 0x08;
pub const OP_PING: u8 = 0x09;
pub const OP_LOGOUT: u8 = 0x0a;
pub const OP_SET_PROFILE: u8 = 0x0b;
pub const OP_CALL_INVITE: u8 = 0x0c;
pub const OP_CALL_ACCEPT: u8 = 0x0d;
pub const OP_CALL_DECLINE: u8 = 0x0e;
pub const OP_CALL_END: u8 = 0x0f;
pub const OP_VOICE: u8 = 0x10;
pub const OP_SEND_VOICE: u8 = 0x11;
pub const OP_SEND_STICKER: u8 = 0x12;

pub const OP_WELCOME: u8 = 0x81;
pub const OP_ERROR: u8 = 0x82;
pub const OP_ROOMS: u8 = 0x83;
pub const OP_JOINED: u8 = 0x84;
pub const OP_MESSAGE: u8 = 0x85;
pub const OP_PRESENCE: u8 = 0x86;
pub const OP_TYPING_EVENT: u8 = 0x87;
pub const OP_HISTORY_EVENT: u8 = 0x88;
pub const OP_PONG: u8 = 0x89;
pub const OP_USERS: u8 = 0x8a;
pub const OP_READ_RECEIPT: u8 = 0x8b;
pub const OP_PROFILE: u8 = 0x8c;
pub const OP_CALL_INVITE_EVENT: u8 = 0x8d;
pub const OP_CALL_ACCEPT_EVENT: u8 = 0x8e;
pub const OP_CALL_DECLINE_EVENT: u8 = 0x8f;
pub const OP_CALL_END_EVENT: u8 = 0x90;
pub const OP_VOICE_EVENT: u8 = 0x91;

/// Кто именно подключился — пригодится серверу для статистики.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Desktop = 1,
    Terminal = 2,
    Console = 3,
    Bot = 4,
}

impl ClientKind {
    pub fn from_u8(v: u8) -> Option<ClientKind> {
        Some(match v {
            1 => ClientKind::Desktop,
            2 => ClientKind::Terminal,
            3 => ClientKind::Console,
            4 => ClientKind::Bot,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ClientKind::Desktop => "desktop",
            ClientKind::Terminal => "terminal",
            ClientKind::Console => "console",
            ClientKind::Bot => "bot",
        }
    }
}

/// Коды ошибок, которые сервер присылает клиенту.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest = 1,
    BadVersion = 2,
    BadInvite = 3,
    BadName = 4,
    BadPassword = 5,
    RateLimited = 6,
    UnknownRoom = 7,
    NotJoined = 8,
    TooLarge = 9,
    ServerFull = 10,
    RoomExists = 11,
    NotAuthed = 12,
}

impl ErrorCode {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<ErrorCode> {
        Some(match v {
            1 => ErrorCode::BadRequest,
            2 => ErrorCode::BadVersion,
            3 => ErrorCode::BadInvite,
            4 => ErrorCode::BadName,
            5 => ErrorCode::BadPassword,
            6 => ErrorCode::RateLimited,
            7 => ErrorCode::UnknownRoom,
            8 => ErrorCode::NotJoined,
            9 => ErrorCode::TooLarge,
            10 => ErrorCode::ServerFull,
            11 => ErrorCode::RoomExists,
            12 => ErrorCode::NotAuthed,
            _ => return None,
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "Сервер не понял запрос",
            ErrorCode::BadVersion => "Версия клиента несовместима с сервером",
            ErrorCode::BadInvite => "Неверный код приглашения",
            ErrorCode::BadName => "Недопустимый ник",
            ErrorCode::BadPassword => "Неверный пароль",
            ErrorCode::RateLimited => "Слишком часто, подожди пару секунд",
            ErrorCode::UnknownRoom => "Такого чата нет",
            ErrorCode::NotJoined => "Чат ещё не открыт",
            ErrorCode::TooLarge => "Слишком длинное сообщение",
            ErrorCode::ServerFull => "На сервере нет свободных мест",
            ErrorCode::RoomExists => "Такой чат уже есть",
            ErrorCode::NotAuthed => "Сначала войди",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientOp {
    Login {
        version: u16,
        name: String,
        password: String,
        invite: String,
        client: ClientKind,
    },
    SendMessage {
        room: u64,
        text: String,
        tag: u32,
    },
    Join {
        room: u64,
    },
    Leave {
        room: u64,
    },
    History {
        room: u64,
        before: u64,
        limit: u16,
    },
    Typing {
        room: u64,
        on: bool,
    },
    /// Открыть личный чат с пользователем (создастся при необходимости).
    OpenDirect {
        user: u64,
    },
    /// «Я дочитал до этого сообщения».
    Read {
        room: u64,
        seq: u64,
    },
    SetProfile {
        handle: String,
        about: String,
        presence: u8,
    },
    CallInvite { to: u64 },
    CallAccept { to: u64 },
    CallDecline { to: u64 },
    CallEnd { to: u64 },
    Voice { to: u64, data: Vec<u8> },
    SendVoice { room: u64, data: Vec<u8>, tag: u32 },
    /// Отправить стикер 1..=MAX_STICKER в чат.
    SendSticker { room: u64, sticker: u8, tag: u32 },
    Ping {
        nonce: u32,
    },
    Logout,
}

impl ClientOp {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::bare();
        match self {
            ClientOp::Login {
                version,
                name,
                password,
                invite,
                client,
            } => {
                w.u8(OP_LOGIN)
                    .u16(*version)
                    .str(name)
                    .str(password)
                    .str(invite)
                    .u8(*client as u8);
            }
            ClientOp::SendMessage { room, text, tag } => {
                w.u8(OP_SEND).u64(*room).str(text).u32(*tag);
            }
            ClientOp::Join { room } => {
                w.u8(OP_JOIN).u64(*room);
            }
            ClientOp::Leave { room } => {
                w.u8(OP_LEAVE).u64(*room);
            }
            ClientOp::History {
                room,
                before,
                limit,
            } => {
                w.u8(OP_HISTORY).u64(*room).u64(*before).u16(*limit);
            }
            ClientOp::Typing { room, on } => {
                w.u8(OP_TYPING).u64(*room).bool(*on);
            }
            ClientOp::OpenDirect { user } => {
                w.u8(OP_OPEN_DIRECT).u64(*user);
            }
            ClientOp::Read { room, seq } => {
                w.u8(OP_READ).u64(*room).u64(*seq);
            }
            ClientOp::SetProfile {
                handle,
                about,
                presence,
            } => {
                w.u8(OP_SET_PROFILE)
                    .str(handle)
                    .str(about)
                    .u8(*presence);
            }
            ClientOp::CallInvite { to } => {
                w.u8(OP_CALL_INVITE).u64(*to);
            }
            ClientOp::CallAccept { to } => {
                w.u8(OP_CALL_ACCEPT).u64(*to);
            }
            ClientOp::CallDecline { to } => {
                w.u8(OP_CALL_DECLINE).u64(*to);
            }
            ClientOp::CallEnd { to } => {
                w.u8(OP_CALL_END).u64(*to);
            }
            ClientOp::Voice { to, data } => {
                w.u8(OP_VOICE).u64(*to).u16(data.len().min(2048) as u16);
                w.bytes(&data[..data.len().min(2048)]);
            }
            ClientOp::SendVoice { room, data, tag } => {
                w.u8(OP_SEND_VOICE).u64(*room).u32(*tag).u16(data.len().min(48 * 1024) as u16);
                w.bytes(&data[..data.len().min(48 * 1024)]);
            }
            ClientOp::SendSticker { room, sticker, tag } => {
                w.u8(OP_SEND_STICKER).u64(*room).u8(*sticker).u32(*tag);
            }
            ClientOp::Ping { nonce } => {
                w.u8(OP_PING).u32(*nonce);
            }
            ClientOp::Logout => {
                w.u8(OP_LOGOUT);
            }
        }
        w.finish()
    }

    pub fn decode(data: &[u8]) -> Result<ClientOp> {
        let mut r = Reader::new(data);
        let op = r.u8()?;
        let decoded = match op {
            OP_LOGIN => {
                let version = r.u16()?;
                if version != crate::PROTOCOL_VERSION {
                    return Err(ProtoError::BadVersion(version));
                }
                let name = r.limited_str(crate::MAX_NAME_BYTES)?;
                let password = r.limited_str(crate::MAX_PASSWORD_BYTES)?;
                let invite = r.limited_str(crate::MAX_INVITE_BYTES)?;
                let client = ClientKind::from_u8(r.u8()?).ok_or(ProtoError::BadValue)?;
                ClientOp::Login {
                    version,
                    name,
                    password,
                    invite,
                    client,
                }
            }
            OP_SEND => {
                let room = r.u64()?;
                let text = r.limited_str(crate::MAX_TEXT_BYTES)?;
                let tag = r.u32()?;
                if text.trim().is_empty() {
                    return Err(ProtoError::BadValue);
                }
                ClientOp::SendMessage { room, text, tag }
            }
            OP_JOIN => ClientOp::Join { room: r.u64()? },
            OP_LEAVE => ClientOp::Leave { room: r.u64()? },
            OP_HISTORY => {
                let room = r.u64()?;
                let before = r.u64()?;
                let limit = r.u16()?.min(MAX_BATCH as u16);
                ClientOp::History {
                    room,
                    before,
                    limit,
                }
            }
            OP_TYPING => ClientOp::Typing {
                room: r.u64()?,
                on: r.bool()?,
            },
            OP_OPEN_DIRECT => ClientOp::OpenDirect { user: r.u64()? },
            OP_READ => ClientOp::Read {
                room: r.u64()?,
                seq: r.u64()?,
            },
            OP_SET_PROFILE => ClientOp::SetProfile {
                handle: r.limited_str(crate::MAX_NAME_BYTES)?,
                about: r.limited_str(crate::MAX_TOPIC_BYTES)?,
                presence: r.u8()?,
            },
            OP_CALL_INVITE => ClientOp::CallInvite { to: r.u64()? },
            OP_CALL_ACCEPT => ClientOp::CallAccept { to: r.u64()? },
            OP_CALL_DECLINE => ClientOp::CallDecline { to: r.u64()? },
            OP_CALL_END => ClientOp::CallEnd { to: r.u64()? },
            OP_SEND_VOICE => {
                let room = r.u64()?;
                let tag = r.u32()?;
                let len = r.u16()? as usize;
                if len == 0 || len > 48 * 1024 {
                    return Err(ProtoError::TooLarge);
                }
                let data = r.bytes(len)?.to_vec();
                ClientOp::SendVoice { room, data, tag }
            }
            OP_SEND_STICKER => {
                let room = r.u64()?;
                let sticker = r.u8()?;
                let tag = r.u32()?;
                if !(1..=crate::MAX_STICKER).contains(&sticker) {
                    return Err(ProtoError::BadValue);
                }
                ClientOp::SendSticker {
                    room,
                    sticker,
                    tag,
                }
            }
            OP_VOICE => {
                let to = r.u64()?;
                let len = r.u16()? as usize;
                if len == 0 || len > 2048 {
                    return Err(ProtoError::TooLarge);
                }
                let data = r.bytes(len)?.to_vec();
                ClientOp::Voice { to, data }
            }
            OP_PING => ClientOp::Ping { nonce: r.u32()? },
            OP_LOGOUT => ClientOp::Logout,
            other => return Err(ProtoError::BadOpcode(other)),
        };
        if !r.is_done() {
            return Err(ProtoError::BadValue);
        }
        Ok(decoded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerOp {
    Welcome {
        user_id: u64,
        server_name: String,
        motd: String,
        rooms: Vec<Room>,
        online: u32,
        users: Vec<User>,
    },
    Error {
        code: u8,
        text: String,
    },
    Rooms {
        rooms: Vec<Room>,
    },
    Users {
        users: Vec<User>,
    },
    Joined {
        room: Room,
        members: Vec<User>,
        messages: Vec<Message>,
        reads: Vec<ReadState>,
    },
    Message {
        msg: Message,
        tag: u32,
    },
    Presence {
        room: u64,
        members: Vec<User>,
    },
    Typing {
        room: u64,
        user_id: u64,
        name: String,
        on: bool,
    },
    History {
        room: u64,
        messages: Vec<Message>,
        complete: bool,
    },
    Pong {
        nonce: u32,
    },
    ReadReceipt {
        room: u64,
        user: u64,
        seq: u64,
    },
    Profile {
        user: User,
    },
    CallInvite { from: User },
    CallAccept { from: User },
    CallDecline { from: User },
    CallEnd { from: User },
    Voice { from: u64, data: Vec<u8> },
}

fn put_rooms(w: &mut Writer, rooms: &[Room]) {
    w.u16(rooms.len().min(MAX_ROOMS) as u16);
    for room in rooms.iter().take(MAX_ROOMS) {
        room.encode(w);
    }
}

fn get_rooms(r: &mut Reader<'_>) -> Result<Vec<Room>> {
    let n = r.u16()? as usize;
    if n > MAX_ROOMS {
        return Err(ProtoError::TooLarge);
    }
    let mut rooms = Vec::with_capacity(n);
    for _ in 0..n {
        rooms.push(Room::decode(r)?);
    }
    Ok(rooms)
}

fn put_users(w: &mut Writer, users: &[User]) {
    w.u16(users.len().min(MAX_MEMBERS) as u16);
    for user in users.iter().take(MAX_MEMBERS) {
        user.encode(w);
    }
}

fn get_users(r: &mut Reader<'_>) -> Result<Vec<User>> {
    let n = r.u16()? as usize;
    if n > MAX_MEMBERS {
        return Err(ProtoError::TooLarge);
    }
    let mut users = Vec::with_capacity(n);
    for _ in 0..n {
        users.push(User::decode(r)?);
    }
    Ok(users)
}

fn put_messages(w: &mut Writer, messages: &[Message]) {
    w.u16(messages.len().min(MAX_BATCH) as u16);
    for m in messages.iter().take(MAX_BATCH) {
        m.encode(w);
    }
}

fn get_messages(r: &mut Reader<'_>) -> Result<Vec<Message>> {
    let n = r.u16()? as usize;
    if n > MAX_BATCH {
        return Err(ProtoError::TooLarge);
    }
    let mut messages = Vec::with_capacity(n);
    for _ in 0..n {
        messages.push(Message::decode(r)?);
    }
    Ok(messages)
}

fn put_reads(w: &mut Writer, reads: &[ReadState]) {
    w.u16(reads.len().min(MAX_MEMBERS) as u16);
    for read in reads.iter().take(MAX_MEMBERS) {
        read.encode(w);
    }
}

fn get_reads(r: &mut Reader<'_>) -> Result<Vec<ReadState>> {
    let n = r.u16()? as usize;
    if n > MAX_MEMBERS {
        return Err(ProtoError::TooLarge);
    }
    let mut reads = Vec::with_capacity(n);
    for _ in 0..n {
        reads.push(ReadState::decode(r)?);
    }
    Ok(reads)
}

impl ServerOp {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::bare();
        match self {
            ServerOp::Welcome {
                user_id,
                server_name,
                motd,
                rooms,
                online,
                users,
            } => {
                w.u8(OP_WELCOME)
                    .u64(*user_id)
                    .str(server_name)
                    .str(motd)
                    .u32(*online);
                put_rooms(&mut w, rooms);
                put_users(&mut w, users);
            }
            ServerOp::Error { code, text } => {
                w.u8(OP_ERROR).u8(*code).str(text);
            }
            ServerOp::Rooms { rooms } => {
                w.u8(OP_ROOMS);
                put_rooms(&mut w, rooms);
            }
            ServerOp::Users { users } => {
                w.u8(OP_USERS);
                put_users(&mut w, users);
            }
            ServerOp::Joined {
                room,
                members,
                messages,
                reads,
            } => {
                w.u8(OP_JOINED).u64(room.id).str(&room.name).str(&room.topic)
                    .u8(room.kind).u16(room.online).u64(room.peer_id);
                put_users(&mut w, members);
                put_messages(&mut w, messages);
                put_reads(&mut w, reads);
            }
            ServerOp::Message { msg, tag } => {
                w.u8(OP_MESSAGE);
                msg.encode(&mut w);
                w.u32(*tag);
            }
            ServerOp::Presence { room, members } => {
                w.u8(OP_PRESENCE).u64(*room);
                put_users(&mut w, members);
            }
            ServerOp::Typing {
                room,
                user_id,
                name,
                on,
            } => {
                w.u8(OP_TYPING_EVENT)
                    .u64(*room)
                    .u64(*user_id)
                    .str(name)
                    .bool(*on);
            }
            ServerOp::History {
                room,
                messages,
                complete,
            } => {
                w.u8(OP_HISTORY_EVENT).u64(*room);
                put_messages(&mut w, messages);
                w.bool(*complete);
            }
            ServerOp::Pong { nonce } => {
                w.u8(OP_PONG).u32(*nonce);
            }
            ServerOp::ReadReceipt { room, user, seq } => {
                w.u8(OP_READ_RECEIPT).u64(*room).u64(*user).u64(*seq);
            }
            ServerOp::Profile { user } => {
                w.u8(OP_PROFILE);
                user.encode(&mut w);
            }
            ServerOp::CallInvite { from } => {
                w.u8(OP_CALL_INVITE_EVENT);
                from.encode(&mut w);
            }
            ServerOp::CallAccept { from } => {
                w.u8(OP_CALL_ACCEPT_EVENT);
                from.encode(&mut w);
            }
            ServerOp::CallDecline { from } => {
                w.u8(OP_CALL_DECLINE_EVENT);
                from.encode(&mut w);
            }
            ServerOp::CallEnd { from } => {
                w.u8(OP_CALL_END_EVENT);
                from.encode(&mut w);
            }
            ServerOp::Voice { from, data } => {
                w.u8(OP_VOICE_EVENT).u64(*from).u16(data.len().min(2048) as u16);
                w.bytes(&data[..data.len().min(2048)]);
            }
        }
        w.finish()
    }

    pub fn decode(data: &[u8]) -> Result<ServerOp> {
        let mut r = Reader::new(data);
        let op = r.u8()?;
        let decoded = match op {
            OP_WELCOME => {
                let user_id = r.u64()?;
                let server_name = r.limited_str(crate::MAX_NAME_BYTES)?;
                let motd = r.limited_str(crate::MAX_TOPIC_BYTES)?;
                let online = r.u32()?;
                let rooms = get_rooms(&mut r)?;
                let users = get_users(&mut r)?;
                ServerOp::Welcome {
                    user_id,
                    server_name,
                    motd,
                    rooms,
                    online,
                    users,
                }
            }
            OP_ERROR => {
                let code = r.u8()?;
                if ErrorCode::from_u8(code).is_none() {
                    return Err(ProtoError::BadValue);
                }
                let text = r.limited_str(256)?;
                ServerOp::Error { code, text }
            }
            OP_ROOMS => ServerOp::Rooms {
                rooms: get_rooms(&mut r)?,
            },
            OP_USERS => ServerOp::Users {
                users: get_users(&mut r)?,
            },
            OP_JOINED => {
                let id = r.u64()?;
                let name = r.limited_str(crate::MAX_ROOM_NAME_BYTES)?;
                let topic = r.limited_str(crate::MAX_TOPIC_BYTES)?;
                let kind = r.u8()?;
                let online = r.u16()?;
                let peer_id = r.u64()?;
                let members = get_users(&mut r)?;
                let messages = get_messages(&mut r)?;
                let reads = get_reads(&mut r)?;
                ServerOp::Joined {
                    room: Room {
                        id,
                        name,
                        topic,
                        kind,
                        online,
                        peer_id,
                    },
                    members,
                    messages,
                    reads,
                }
            }
            OP_MESSAGE => {
                let msg = Message::decode(&mut r)?;
                let tag = r.u32()?;
                ServerOp::Message { msg, tag }
            }
            OP_PRESENCE => {
                let room = r.u64()?;
                let members = get_users(&mut r)?;
                ServerOp::Presence { room, members }
            }
            OP_TYPING_EVENT => ServerOp::Typing {
                room: r.u64()?,
                user_id: r.u64()?,
                name: r.limited_str(crate::MAX_NAME_BYTES)?,
                on: r.bool()?,
            },
            OP_HISTORY_EVENT => {
                let room = r.u64()?;
                let messages = get_messages(&mut r)?;
                let complete = r.bool()?;
                ServerOp::History {
                    room,
                    messages,
                    complete,
                }
            }
            OP_PONG => ServerOp::Pong { nonce: r.u32()? },
            OP_READ_RECEIPT => ServerOp::ReadReceipt {
                room: r.u64()?,
                user: r.u64()?,
                seq: r.u64()?,
            },
            OP_PROFILE => ServerOp::Profile {
                user: User::decode(&mut r)?,
            },
            OP_CALL_INVITE_EVENT => ServerOp::CallInvite {
                from: User::decode(&mut r)?,
            },
            OP_CALL_ACCEPT_EVENT => ServerOp::CallAccept {
                from: User::decode(&mut r)?,
            },
            OP_CALL_DECLINE_EVENT => ServerOp::CallDecline {
                from: User::decode(&mut r)?,
            },
            OP_CALL_END_EVENT => ServerOp::CallEnd {
                from: User::decode(&mut r)?,
            },
            OP_VOICE_EVENT => {
                let from = r.u64()?;
                let len = r.u16()? as usize;
                if len == 0 || len > 2048 {
                    return Err(ProtoError::TooLarge);
                }
                let data = r.bytes(len)?.to_vec();
                ServerOp::Voice { from, data }
            }
            other => return Err(ProtoError::BadOpcode(other)),
        };
        if !r.is_done() {
            return Err(ProtoError::BadValue);
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: u64) -> Room {
        Room {
            id,
            name: "Аня".into(),
            topic: String::new(),
            kind: crate::model::ROOM_DM,
            online: 1,
            peer_id: 2,
        }
    }

    #[test]
    fn login_roundtrip() {
        let op = ClientOp::Login {
            version: crate::PROTOCOL_VERSION,
            name: "Аня".into(),
            password: "12345".into(),
            invite: "krug".into(),
            client: ClientKind::Desktop,
        };
        assert_eq!(ClientOp::decode(&op.encode()).unwrap(), op);
    }

    #[test]
    fn direct_and_read_roundtrip() {
        let open = ClientOp::OpenDirect { user: 7 };
        assert_eq!(ClientOp::decode(&open.encode()).unwrap(), open);
        let read = ClientOp::Read { room: 3, seq: 42 };
        assert_eq!(ClientOp::decode(&read.encode()).unwrap(), read);
    }

    #[test]
    fn send_sticker_roundtrip_and_limits() {
        let op = ClientOp::SendSticker {
            room: 3,
            sticker: 2,
            tag: 77,
        };
        assert_eq!(ClientOp::decode(&op.encode()).unwrap(), op);
        for sticker in 1..=crate::MAX_STICKER {
            let op = ClientOp::SendSticker {
                room: 1,
                sticker,
                tag: 0,
            };
            assert_eq!(ClientOp::decode(&op.encode()).unwrap(), op);
        }
        let mut bad = op.encode();
        bad[9] = 0;
        assert!(ClientOp::decode(&bad).is_err());
        bad[9] = crate::MAX_STICKER + 1;
        assert!(ClientOp::decode(&bad).is_err());
    }

    #[test]
    fn welcome_roundtrip() {
        let op = ServerOp::Welcome {
            user_id: 1,
            server_name: "Круг".into(),
            motd: "свои".into(),
            rooms: alloc::vec![room(1)],
            online: 2,
            users: alloc::vec![User {
                id: 2,
                name: "Боб".into(),
                handle: "bob".into(),
                about: String::new(),
                status: 0,
                presence: 0,
            }],
        };
        assert_eq!(ServerOp::decode(&op.encode()).unwrap(), op);
    }

    #[test]
    fn joined_roundtrip() {
        let op = ServerOp::Joined {
            room: room(3),
            members: alloc::vec![],
            messages: alloc::vec![],
            reads: alloc::vec![ReadState { user: 2, seq: 5 }],
        };
        assert_eq!(ServerOp::decode(&op.encode()).unwrap(), op);
    }

    #[test]
    fn users_and_receipt() {
        let users = ServerOp::Users {
            users: alloc::vec![User {
                id: 2,
                name: "Боб".into(),
                handle: "bob".into(),
                about: String::new(),
                status: 1,
                presence: 1,
            }],
        };
        assert_eq!(ServerOp::decode(&users.encode()).unwrap(), users);
        let receipt = ServerOp::ReadReceipt {
            room: 1,
            user: 2,
            seq: 9,
        };
        assert_eq!(ServerOp::decode(&receipt.encode()).unwrap(), receipt);
    }

    #[test]
    fn message_roundtrip() {
        let op = ServerOp::Message {
            msg: Message {
                room: 1,
                seq: 9,
                ts_ms: 1,
                author_id: 2,
                author: "b".into(),
                text: "привет".into(),
                voice: false,
                sticker: 0,
                audio: alloc::vec![],
            },
            tag: 3,
        };
        assert_eq!(ServerOp::decode(&op.encode()).unwrap(), op);
    }

    #[test]
    fn sticker_message_roundtrip() {
        let op = ServerOp::Message {
            msg: Message {
                room: 1,
                seq: 10,
                ts_ms: 2,
                author_id: 2,
                author: "b".into(),
                text: String::new(),
                voice: false,
                sticker: 4,
                audio: alloc::vec![],
            },
            tag: 0,
        };
        let decoded = ServerOp::decode(&op.encode()).unwrap();
        assert_eq!(decoded, op);
        if let ServerOp::Message { msg, .. } = decoded {
            assert_eq!(msg.sticker, 4);
            assert!(msg.text.is_empty());
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(ClientOp::decode(&[0xff, 1, 2]).is_err());
        assert!(ClientOp::decode(&[]).is_err());
        assert!(ServerOp::decode(&[0x00]).is_err());
    }
}
