//! Хранилище сервера: пользователи, комнаты и история сообщений.
//!
//! Форматы намеренно простые и бинарные — без serde и баз данных:
//! `users.bin` (записи с длиной), `rooms.tsv` и `messages/<id>.bin`.
//! Такой сервер поднимается на любой машине с одним бинарником.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use krug_core::model::Message;
use krug_core::Reader;

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: u64,
    pub name: String,
    pub salt: [u8; 16],
    pub hash: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub id: u64,
    pub handle: String,
    pub about: String,
    pub presence: u8,
}

#[derive(Debug, Clone)]
pub struct RoomRecord {
    pub id: u64,
    pub name: String,
    pub topic: String,
    pub kind: u8,
    pub members: Vec<u64>,
}

#[derive(Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> io::Result<Store> {
        fs::create_dir_all(root.join("messages"))?;
        Ok(Store {
            root: root.to_path_buf(),
        })
    }

    fn users_path(&self) -> PathBuf {
        self.root.join("users.bin")
    }

    fn rooms_path(&self) -> PathBuf {
        self.root.join("rooms.tsv")
    }

    fn profiles_path(&self) -> PathBuf {
        self.root.join("profiles.tsv")
    }

    pub fn load_profiles(&self) -> io::Result<Vec<ProfileRecord>> {
        let text = match fs::read_to_string(self.profiles_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let mut parts = line.split('\t');
            let id = match parts.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let handle = parts.next().unwrap_or("").to_string();
            let about = parts.next().unwrap_or("").to_string();
            let presence = parts
                .next()
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0);
            out.push(ProfileRecord {
                id,
                handle,
                about,
                presence,
            });
        }
        Ok(out)
    }

    pub fn save_profiles(&self, profiles: &[ProfileRecord]) -> io::Result<()> {
        let mut text = String::new();
        for profile in profiles {
            let handle = profile.handle.replace(['\t', '\n'], " ");
            let about = profile.about.replace(['\t', '\n'], " ");
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                profile.id, handle, about, profile.presence
            ));
        }
        let tmp = self.profiles_path().with_extension("tsv.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, self.profiles_path())
    }

    fn messages_path(&self, room: u64) -> PathBuf {
        self.root.join("messages").join(format!("{room}.bin"))
    }

    pub fn load_users(&self) -> io::Result<Vec<UserRecord>> {
        let path = self.users_path();
        let mut data = Vec::new();
        match fs::File::open(&path) {
            Ok(mut f) => {
                f.read_to_end(&mut data)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= data.len() {
            let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            pos += 4;
            if pos + len > data.len() {
                break;
            }
            let rec = &data[pos..pos + len];
            pos += len;
            let mut r = Reader::new(rec);
            let id = match r.u64() {
                Ok(v) => v,
                Err(_) => break,
            };
            let name_len = match r.u8() {
                Ok(v) => v as usize,
                Err(_) => break,
            };
            let name = match r.bytes(name_len).and_then(|b| {
                core::str::from_utf8(b)
                    .map(|s| s.to_string())
                    .map_err(|_| krug_core::ProtoError::BadUtf8)
            }) {
                Ok(v) => v,
                Err(_) => break,
            };
            let salt_bytes = match r.bytes(16) {
                Ok(v) => v,
                Err(_) => break,
            };
            let hash_bytes = match r.bytes(32) {
                Ok(v) => v,
                Err(_) => break,
            };
            let mut salt = [0u8; 16];
            salt.copy_from_slice(salt_bytes);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            out.push(UserRecord {
                id,
                name,
                salt,
                hash,
            });
        }
        Ok(out)
    }

    pub fn append_user(&self, user: &UserRecord) -> io::Result<()> {
        let mut payload = Vec::with_capacity(64);
        payload.extend_from_slice(&user.id.to_le_bytes());
        payload.push(user.name.len() as u8);
        payload.extend_from_slice(user.name.as_bytes());
        payload.extend_from_slice(&user.salt);
        payload.extend_from_slice(&user.hash);
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.users_path())?;
        f.write_all(&(payload.len() as u32).to_le_bytes())?;
        f.write_all(&payload)?;
        f.flush()
    }

    pub fn load_rooms(&self) -> io::Result<Vec<RoomRecord>> {
        let text = match fs::read_to_string(self.rooms_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let mut parts = line.split('\t');
            let id = match parts.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(v) => v,
                None => continue,
            };
            let kind = parts
                .next()
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0);
            let name = parts.next().unwrap_or("комната").to_string();
            let topic = parts.next().unwrap_or("").to_string();
            let members = parts
                .next()
                .unwrap_or("")
                .split(',')
                .filter_map(|part| part.trim().parse::<u64>().ok())
                .collect();
            out.push(RoomRecord {
                id,
                name,
                topic,
                kind,
                members,
            });
        }
        Ok(out)
    }

    pub fn save_rooms(&self, rooms: &[RoomRecord]) -> io::Result<()> {
        let mut text = String::new();
        for room in rooms {
            let name = room.name.replace(['\t', '\n'], " ");
            let topic = room.topic.replace(['\t', '\n'], " ");
            let members = room
                .members
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                room.id, room.kind, name, topic, members
            ));
        }
        let tmp = self.rooms_path().with_extension("tsv.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, self.rooms_path())
    }

    pub fn load_messages(&self, room: u64, limit: usize) -> io::Result<Vec<Message>> {
        let mut data = Vec::new();
        match fs::File::open(self.messages_path(room)) {
            Ok(mut f) => {
                f.read_to_end(&mut data)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= data.len() {
            let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            pos += 4;
            if pos + len > data.len() {
                break;
            }
            let rec = &data[pos..pos + len];
            pos += len;
            if let Ok(msg) = Message::from_record(rec) {
                out.push(msg);
            }
        }
        if out.len() > limit {
            out.drain(..out.len() - limit);
        }
        Ok(out)
    }

    pub fn append_message(&self, msg: &Message) -> io::Result<()> {
        let record = msg.to_record();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.messages_path(msg.room))?;
        f.write_all(&(record.len() as u32).to_le_bytes())?;
        f.write_all(&record)?;
        f.flush()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Сколько последних сообщений держим в памяти и в стартовой истории.
pub const HISTORY_LIMIT: usize = 2000;
