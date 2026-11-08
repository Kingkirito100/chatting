//! Модель данных «Круга» и её бинарное представление.

use alloc::string::String;

use crate::wire::{ProtoError, Reader, Result, Writer};

pub type UserId = u64;
pub type RoomId = u64;

pub const ROOM_TEXT: u8 = 0;
pub const ROOM_VOICE: u8 = 1;
/// Личный чат двух пользователей.
pub const ROOM_DM: u8 = 2;

pub const STATUS_ONLINE: u8 = 0;
pub const STATUS_AWAY: u8 = 1;
pub const STATUS_DND: u8 = 2;

/// Комната (канал) — текстовый или голосовой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: RoomId,
    pub name: String,
    pub topic: String,
    pub kind: u8,
    pub online: u16,
    /// Для личного чата — собеседник.
    pub peer_id: UserId,
}

impl Room {
    pub fn encode(&self, w: &mut Writer) {
        w.u64(self.id)
            .str(&self.name)
            .str(&self.topic)
            .u8(self.kind)
            .u16(self.online)
            .u64(self.peer_id);
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Room> {
        Ok(Room {
            id: r.u64()?,
            name: r.limited_str(crate::MAX_ROOM_NAME_BYTES)?,
            topic: r.limited_str(crate::MAX_TOPIC_BYTES)?,
            kind: r.u8()?,
            online: r.u16()?,
            peer_id: r.u64()?,
        })
    }
}

/// Участник комнаты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub name: String,
    /// Уникальный ник без @.
    pub handle: String,
    /// Короткий статус.
    pub about: String,
    /// 0 — в сети, 1 — офлайн.
    pub status: u8,
    /// Выбранный статус: 0 в сети, 1 не активен, 2 не беспокоить, 3 невидимый.
    pub presence: u8,
}

impl User {
    pub fn encode(&self, w: &mut Writer) {
        w.u64(self.id)
            .str(&self.name)
            .str(&self.handle)
            .str(&self.about)
            .u8(self.status)
            .u8(self.presence);
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<User> {
        Ok(User {
            id: r.u64()?,
            name: r.limited_str(crate::MAX_NAME_BYTES)?,
            handle: r.limited_str(crate::MAX_NAME_BYTES)?,
            about: r.limited_str(crate::MAX_TOPIC_BYTES)?,
            status: r.u8()?,
            presence: r.u8()?,
        })
    }
}

/// Отметка о прочтении: кто до какого сообщения дочитал.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadState {
    pub user: UserId,
    pub seq: u64,
}

impl ReadState {
    pub fn encode(&self, w: &mut Writer) {
        w.u64(self.user).u64(self.seq);
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<ReadState> {
        Ok(ReadState {
            user: r.u64()?,
            seq: r.u64()?,
        })
    }
}

/// Сообщение в комнате.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub room: RoomId,
    pub seq: u64,
    /// Время отправки, миллисекунды Unix.
    pub ts_ms: u64,
    pub author_id: UserId,
    pub author: String,
    pub text: String,
    /// Голосовое сообщение (PCM 16 кГц, i16 LE).
    pub voice: bool,
    /// Стикер: 0 — нет, 1..=MAX_STICKER — номер картинки.
    pub sticker: u8,
    pub audio: Vec<u8>,
}

impl Message {
    pub fn encode(&self, w: &mut Writer) {
        w.u64(self.room)
            .u64(self.seq)
            .u64(self.ts_ms)
            .u64(self.author_id)
            .str(&self.author)
            .str(&self.text)
            .bool(self.voice)
            .u8(self.sticker)
            .u16(self.audio.len().min(48 * 1024) as u16);
        w.bytes(&self.audio[..self.audio.len().min(48 * 1024)]);
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Message> {
        let room = r.u64()?;
        let seq = r.u64()?;
        let ts_ms = r.u64()?;
        let author_id = r.u64()?;
        let author = r.limited_str(crate::MAX_NAME_BYTES)?;
        let text = r.limited_str(crate::MAX_TEXT_BYTES)?;
        let voice = r.bool()?;
        let sticker = r.u8()?;
        if sticker > crate::MAX_STICKER {
            return Err(ProtoError::BadValue);
        }
        let len = r.u16()? as usize;
        if len > 48 * 1024 {
            return Err(ProtoError::TooLarge);
        }
        let audio = r.bytes(len)?.to_vec();
        if text.is_empty() && !voice && sticker == 0 {
            return Err(ProtoError::BadValue);
        }
        Ok(Message {
            room,
            seq,
            ts_ms,
            author_id,
            author,
            text,
            voice,
            sticker,
            audio,
        })
    }

    /// Компактная запись для файла истории (без кода операции).
    pub fn to_record(&self) -> alloc::vec::Vec<u8> {
        let mut w = Writer::bare();
        self.encode(&mut w);
        w.finish()
    }

    pub fn from_record(bytes: &[u8]) -> Result<Message> {
        let mut r = Reader::new(bytes);
        Message::decode(&mut r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let m = Message {
            room: 3,
            seq: 42,
            ts_ms: 1_700_000_000_000,
            author_id: 9,
            author: "Аня".into(),
            text: "привет, го в игру".into(),
            voice: false,
            sticker: 0,
            audio: Vec::new(),
        };
        let rec = m.to_record();
        assert_eq!(Message::from_record(&rec).unwrap(), m);
    }

    #[test]
    fn sticker_message_roundtrip() {
        let m = Message {
            room: 3,
            seq: 43,
            ts_ms: 1_700_000_001_000,
            author_id: 9,
            author: "Аня".into(),
            text: String::new(),
            voice: false,
            sticker: 2,
            audio: Vec::new(),
        };
        let rec = m.to_record();
        let decoded = Message::from_record(&rec).unwrap();
        assert_eq!(decoded, m);
        assert_eq!(decoded.sticker, 2);
        assert!(decoded.text.is_empty());
    }

    #[test]
    fn empty_message_needs_voice_or_sticker() {
        let mut w = Writer::bare();
        w.u64(1)
            .u64(1)
            .u64(1)
            .u64(1)
            .str("Аня")
            .str("")
            .bool(false)
            .u8(0)
            .u16(0);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert!(Message::decode(&mut r).is_err());
    }

    #[test]
    fn room_roundtrip() {
        let room = Room {
            id: 1,
            name: "general".into(),
            topic: "общий чат".into(),
            kind: ROOM_TEXT,
            online: 4,
            peer_id: 0,
        };
        let mut w = Writer::bare();
        room.encode(&mut w);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(Room::decode(&mut r).unwrap(), room);
    }
}
