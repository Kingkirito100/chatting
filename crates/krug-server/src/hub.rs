//! Состояние сервера: пользователи, личные чаты, прочтения.
//!
//! Никаких каналов и лент: только пары людей и их переписка.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use krug_core::model::{Message, ReadState, Room, User, ROOM_DM, STATUS_ONLINE};
use krug_core::{
    sanitize_text, valid_name, valid_password, ClientOp, ErrorCode, ServerOp, MAX_BATCH,
    MAX_STICKER, MAX_TEXT_BYTES,
};

use crate::store::{ProfileRecord, RoomRecord, Store, UserRecord, HISTORY_LIMIT};
use crate::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Continue,
    Close,
}

struct Conn {
    tx: mpsc::Sender<Vec<u8>>,
    user_id: Option<u64>,
    name: String,
    /// Открытые в интерфейсе чаты (для истории/присутствия).
    rooms: HashSet<u64>,
    tokens: f32,
    last_refill: Instant,
    last_typing: Option<Instant>,
}

#[derive(Debug, Clone)]
struct Profile {
    handle: String,
    about: String,
    presence: u8,
}

struct RoomState {
    room: Room,
    messages: VecDeque<Message>,
    next_seq: u64,
    members: Vec<u64>,
    reads: HashMap<u64, u64>,
}

pub struct Shared {
    inner: Mutex<Inner>,
}

struct Inner {
    cfg: Config,
    store: Store,
    users_by_name: HashMap<String, UserRecord>,
    users_by_id: HashMap<u64, UserRecord>,
    rooms: BTreeMap<u64, RoomState>,
    profiles: HashMap<u64, Profile>,
    handles: HashMap<String, u64>,
    conns: HashMap<u64, Conn>,
    next_user: u64,
    next_room: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Shared {
    pub fn new(cfg: Config) -> std::io::Result<Shared> {
        let store = Store::open(&cfg.data_dir)?;

        let mut users_by_name = HashMap::new();
        let mut users_by_id = HashMap::new();
        let mut next_user = 1u64;
        for user in store.load_users()? {
            next_user = next_user.max(user.id + 1);
            users_by_id.insert(user.id, user.clone());
            users_by_name.insert(user.name.to_lowercase(), user);
        }

        let mut profiles: HashMap<u64, Profile> = HashMap::new();
        let mut handles: HashMap<String, u64> = HashMap::new();
        for record in store.load_profiles()? {
            if record.handle.is_empty() {
                continue;
            }
            handles.insert(record.handle.clone(), record.id);
            profiles.insert(
                record.id,
                Profile {
                    handle: record.handle,
                    about: record.about,
                    presence: record.presence.min(3),
                },
            );
        }
        for id in users_by_id.keys() {
            if !profiles.contains_key(id) {
                let mut handle = format!("user{id}");
                while handles.contains_key(&handle) {
                    handle.push('0');
                }
                handles.insert(handle.clone(), *id);
                profiles.insert(
                    *id,
                    Profile {
                        handle,
                        about: String::new(),
                        presence: 0,
                    },
                );
            }
        }

        let mut rooms = BTreeMap::new();
        let mut next_room = 1u64;
        for rec in store.load_rooms()? {
            next_room = next_room.max(rec.id + 1);
            let mut messages = store.load_messages(rec.id, HISTORY_LIMIT)?;
            let next_seq = messages.last().map(|m| m.seq + 1).unwrap_or(1);
            let deque: VecDeque<Message> = messages.drain(..).collect();
            let mut reads = HashMap::new();
            for member in &rec.members {
                reads.insert(*member, 0);
            }
            rooms.insert(
                rec.id,
                RoomState {
                    room: Room {
                        id: rec.id,
                        name: if rec.kind == ROOM_DM {
                            String::new()
                        } else {
                            rec.name
                        },
                        topic: rec.topic,
                        kind: rec.kind,
                        online: 0,
                        peer_id: 0,
                    },
                    messages: deque,
                    next_seq,
                    members: rec.members,
                    reads,
                },
            );
        }

        Ok(Shared {
            inner: Mutex::new(Inner {
                cfg,
                store,
                users_by_name,
                users_by_id,
                rooms,
                profiles,
                handles,
                conns: HashMap::new(),
                next_user,
                next_room,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn attach(&self, conn_id: u64, tx: mpsc::Sender<Vec<u8>>) {
        let mut inner = self.lock();
        inner.conns.insert(
            conn_id,
            Conn {
                tx,
                user_id: None,
                name: String::new(),
                rooms: HashSet::new(),
                tokens: 6.0,
                last_refill: Instant::now(),
                last_typing: None,
            },
        );
    }

    pub fn bad_request(&self, conn_id: u64, text: &str) {
        let inner = self.lock();
        inner.error(conn_id, ErrorCode::BadRequest, Some(text));
    }

    pub fn detach(&self, conn_id: u64) {
        let mut inner = self.lock();
        let Some(conn) = inner.conns.remove(&conn_id) else {
            return;
        };
        if conn.user_id.is_some() {
            inner.broadcast_users();
        }
    }

    pub fn handle(&self, conn_id: u64, op: ClientOp) -> Action {
        let mut inner = self.lock();
        if !inner.conns.contains_key(&conn_id) {
            return Action::Close;
        }
        let authed = inner.conns.get(&conn_id).and_then(|c| c.user_id).is_some();

        match op {
            ClientOp::Login {
                name,
                password,
                invite,
                ..
            } => {
                if authed {
                    inner.error(conn_id, ErrorCode::BadRequest, None);
                    return Action::Continue;
                }
                match inner.login(conn_id, &name, &password, &invite) {
                    Ok(user_id) => {
                        let users = inner.users_list();
                        let welcome = ServerOp::Welcome {
                            user_id,
                            server_name: inner.cfg.server_name.clone(),
                            motd: inner.cfg.motd.clone(),
                            rooms: inner.room_list(user_id),
                            online: users
                                .iter()
                                .filter(|u| u.status == STATUS_ONLINE)
                                .count() as u32,
                            users,
                        };
                        inner.send_to(conn_id, &welcome.encode());
                        inner.broadcast_users();
                        println!("[krug] {} в сети", inner.conn_name(conn_id));
                        Action::Continue
                    }
                    Err(code) => {
                        inner.error(conn_id, code, None);
                        Action::Close
                    }
                }
            }
            _ if !authed => {
                inner.error(conn_id, ErrorCode::NotAuthed, None);
                Action::Close
            }
            ClientOp::Ping { nonce } => {
                inner.send_to(conn_id, &ServerOp::Pong { nonce }.encode());
                Action::Continue
            }
            ClientOp::OpenDirect { user } => {
                if let Err(code) = inner.open_direct(conn_id, user) {
                    inner.error(conn_id, code, None);
                }
                Action::Continue
            }
            ClientOp::Join { room } => {
                if let Err(code) = inner.join_room(conn_id, room) {
                    inner.error(conn_id, code, None);
                }
                Action::Continue
            }
            ClientOp::Leave { room } => {
                if let Some(conn) = inner.conns.get_mut(&conn_id) {
                    conn.rooms.remove(&room);
                }
                Action::Continue
            }
            ClientOp::SendVoice { room, data, tag } => {
                let Some((uid, name)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                if !inner.take_message_token(conn_id) {
                    inner.error(conn_id, ErrorCode::RateLimited, None);
                    return Action::Continue;
                }
                if !inner.is_member(room, uid) {
                    inner.error(conn_id, ErrorCode::NotJoined, None);
                    return Action::Continue;
                }
                if data.is_empty() || data.len() > 48 * 1024 {
                    inner.error(conn_id, ErrorCode::TooLarge, None);
                    return Action::Continue;
                }
                if let Some(msg) = inner.store_voice(uid, &name, room, data) {
                    let frame = ServerOp::Message { msg, tag }.encode();
                    inner.broadcast_participants(room, &frame, None);
                }
                Action::Continue
            }
            ClientOp::SendSticker { room, sticker, tag } => {
                let Some((uid, name)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                if !inner.take_message_token(conn_id) {
                    inner.error(conn_id, ErrorCode::RateLimited, None);
                    return Action::Continue;
                }
                if !inner.is_member(room, uid) {
                    inner.error(conn_id, ErrorCode::NotJoined, None);
                    return Action::Continue;
                }
                if !(1..=MAX_STICKER).contains(&sticker) {
                    inner.error(conn_id, ErrorCode::BadRequest, Some("Неизвестный стикер"));
                    return Action::Continue;
                }
                if let Some(msg) = inner.store_sticker(uid, &name, room, sticker) {
                    let frame = ServerOp::Message { msg, tag }.encode();
                    inner.broadcast_participants(room, &frame, None);
                }
                Action::Continue
            }
            ClientOp::SendMessage { room, text, tag } => {
                if !inner.take_message_token(conn_id) {
                    inner.error(conn_id, ErrorCode::RateLimited, None);
                    return Action::Continue;
                }
                let Some((uid, name)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                if !inner.is_member(room, uid) {
                    inner.error(conn_id, ErrorCode::NotJoined, None);
                    return Action::Continue;
                }
                let text = sanitize_text(&text);
                if text.is_empty() {
                    return Action::Continue;
                }
                if text.len() > MAX_TEXT_BYTES {
                    inner.error(conn_id, ErrorCode::TooLarge, None);
                    return Action::Continue;
                }
                match inner.store_message(uid, &name, room, text) {
                    Some(msg) => {
                        let frame = ServerOp::Message { msg, tag }.encode();
                        inner.broadcast_participants(room, &frame, None);
                    }
                    None => inner.error(conn_id, ErrorCode::UnknownRoom, None),
                }
                Action::Continue
            }
            ClientOp::History {
                room,
                before,
                limit,
            } => {
                let Some((uid, _)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                if !inner.is_member(room, uid) {
                    inner.error(conn_id, ErrorCode::NotJoined, None);
                    return Action::Continue;
                }
                let limit = (limit as usize).clamp(1, MAX_BATCH);
                let (messages, complete) = inner.history(room, before, limit);
                let frame = ServerOp::History {
                    room,
                    messages,
                    complete,
                }
                .encode();
                inner.send_to(conn_id, &frame);
                Action::Continue
            }
            ClientOp::Typing { room, on } => {
                if !inner.take_typing_token(conn_id) {
                    return Action::Continue;
                }
                let Some((uid, name)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                if !inner.is_member(room, uid) {
                    return Action::Continue;
                }
                let frame = ServerOp::Typing {
                    room,
                    user_id: uid,
                    name,
                    on,
                }
                .encode();
                inner.broadcast_participants(room, &frame, Some(conn_id));
                Action::Continue
            }
            ClientOp::Read { room, seq } => {
                let Some((uid, _)) = inner.identity(conn_id) else {
                    return Action::Close;
                };
                inner.mark_read(room, uid, seq);
                Action::Continue
            }
            ClientOp::SetProfile {
                handle,
                about,
                presence,
            } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    if let Err(text) = inner.set_profile(uid, &handle, &about, presence) {
                        inner.error(conn_id, ErrorCode::BadRequest, Some(&text));
                    }
                }
                Action::Continue
            }
            ClientOp::CallInvite { to } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    let from = inner.public_user(uid);
                    inner.send_to_user(to, &ServerOp::CallInvite { from }.encode());
                }
                Action::Continue
            }
            ClientOp::CallAccept { to } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    let from = inner.public_user(uid);
                    inner.send_to_user(to, &ServerOp::CallAccept { from }.encode());
                }
                Action::Continue
            }
            ClientOp::CallDecline { to } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    let from = inner.public_user(uid);
                    inner.send_to_user(to, &ServerOp::CallDecline { from }.encode());
                }
                Action::Continue
            }
            ClientOp::CallEnd { to } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    let from = inner.public_user(uid);
                    inner.send_to_user(to, &ServerOp::CallEnd { from }.encode());
                }
                Action::Continue
            }
            ClientOp::Voice { to, data } => {
                if let Some((uid, _)) = inner.identity(conn_id) {
                    let frame = ServerOp::Voice { from: uid, data }.encode();
                    inner.send_to_user(to, &frame);
                }
                Action::Continue
            }
            ClientOp::Logout => Action::Close,
        }
    }
}

impl Inner {
    fn conn_name(&self, conn_id: u64) -> String {
        self.conns
            .get(&conn_id)
            .map(|c| {
                if c.name.is_empty() {
                    "кто-то".to_string()
                } else {
                    c.name.clone()
                }
            })
            .unwrap_or_else(|| "кто-то".to_string())
    }

    fn identity(&self, conn_id: u64) -> Option<(u64, String)> {
        let conn = self.conns.get(&conn_id)?;
        Some((conn.user_id?, conn.name.clone()))
    }

    fn name_of(&self, user: u64) -> String {
        self.users_by_id
            .get(&user)
            .map(|u| u.name.clone())
            .unwrap_or_default()
    }

    fn is_online(&self, user: u64) -> bool {
        self.conns
            .values()
            .any(|c| c.user_id == Some(user))
    }

    fn is_member(&self, room: u64, user: u64) -> bool {
        self.rooms
            .get(&room)
            .map(|s| s.members.contains(&user))
            .unwrap_or(false)
    }

    fn send_to_user(&self, user: u64, frame: &[u8]) {
        for conn in self.conns.values() {
            if conn.user_id == Some(user) {
                let _ = conn.tx.send(frame.to_vec());
            }
        }
    }

    fn send_to(&self, conn_id: u64, frame: &[u8]) {
        if let Some(conn) = self.conns.get(&conn_id) {
            let _ = conn.tx.send(frame.to_vec());
        }
    }

    fn broadcast_participants(&self, room: u64, frame: &[u8], except: Option<u64>) {
        let Some(state) = self.rooms.get(&room) else {
            return;
        };
        for (id, conn) in &self.conns {
            if Some(*id) == except {
                continue;
            }
            if let Some(uid) = conn.user_id {
                if state.members.contains(&uid) {
                    let _ = conn.tx.send(frame.to_vec());
                }
            }
        }
    }

    fn broadcast_users(&self) {
        let frame = ServerOp::Users {
            users: self.users_list(),
        }
        .encode();
        for conn in self.conns.values() {
            if conn.user_id.is_some() {
                let _ = conn.tx.send(frame.clone());
            }
        }
    }

    fn broadcast_rooms(&self) {
        let targets: Vec<(u64, u64)> = self
            .conns
            .iter()
            .filter_map(|(id, conn)| conn.user_id.map(|uid| (*id, uid)))
            .collect();
        for (conn_id, user_id) in targets {
            let frame = ServerOp::Rooms {
                rooms: self.room_list(user_id),
            }
            .encode();
            self.send_to(conn_id, &frame);
        }
    }

    fn public_user(&self, id: u64) -> User {
        let profile = self.profiles.get(&id);
        let presence = profile.map(|p| p.presence).unwrap_or(0);
        User {
            id,
            name: self.name_of(id),
            handle: profile.map(|p| p.handle.clone()).unwrap_or_default(),
            about: profile.map(|p| p.about.clone()).unwrap_or_default(),
            status: if self.is_online(id) && presence != 3 {
                STATUS_ONLINE
            } else {
                1
            },
            presence,
        }
    }

    fn users_list(&self) -> Vec<User> {
        let mut users: Vec<User> = self
            .users_by_id
            .keys()
            .map(|id| self.public_user(*id))
            .collect();
        users.sort_by(|a, b| {
            b.status
                .cmp(&a.status)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        users
    }

    fn room_list(&self, viewer: u64) -> Vec<Room> {
        let mut rooms: Vec<Room> = self
            .rooms
            .values()
            .filter(|state| state.room.kind == ROOM_DM && state.members.contains(&viewer))
            .map(|state| {
                let peer = state
                    .members
                    .iter()
                    .copied()
                    .find(|id| *id != viewer)
                    .unwrap_or(0);
                Room {
                    id: state.room.id,
                    name: self.name_of(peer),
                    topic: String::new(),
                    kind: ROOM_DM,
                    online: if self.is_online(peer) { 1 } else { 0 },
                    peer_id: peer,
                }
            })
            .collect();
        rooms.sort_by_key(|r| r.name.to_lowercase());
        rooms
    }

    fn room_view(&self, room: u64, viewer: u64) -> Option<Room> {
        let state = self.rooms.get(&room)?;
        let peer = state
            .members
            .iter()
            .copied()
            .find(|id| *id != viewer)
            .unwrap_or(0);
        Some(Room {
            id: state.room.id,
            name: self.name_of(peer),
            topic: String::new(),
            kind: ROOM_DM,
            online: if self.is_online(peer) { 1 } else { 0 },
            peer_id: peer,
        })
    }

    fn member_users(&self, room: u64) -> Vec<User> {
        let Some(state) = self.rooms.get(&room) else {
            return Vec::new();
        };
        state.members.iter().map(|id| self.public_user(*id)).collect()
    }

    fn error(&self, conn_id: u64, code: ErrorCode, text: Option<&str>) {
        let text = text.unwrap_or_else(|| code.text()).to_string();
        let frame = ServerOp::Error {
            code: code.as_u8(),
            text,
        }
        .encode();
        self.send_to(conn_id, &frame);
    }

    fn login(
        &mut self,
        conn_id: u64,
        name: &str,
        password: &str,
        invite: &str,
    ) -> Result<u64, ErrorCode> {
        let name = name.trim();
        if !valid_name(name) {
            return Err(ErrorCode::BadName);
        }
        if !valid_password(password) {
            return Err(ErrorCode::BadPassword);
        }
        let key = name.to_lowercase();
        let user = if let Some(existing) = self.users_by_name.get(&key) {
            if !krug_core::sha256::verify_password(&existing.salt, password, &existing.hash) {
                return Err(ErrorCode::BadPassword);
            }
            existing.clone()
        } else {
            if !self.cfg.invite.is_empty()
                && !krug_core::constant_time_eq(&self.cfg.invite, invite)
            {
                return Err(ErrorCode::BadInvite);
            }
            if self.users_by_name.len() >= self.cfg.max_users {
                return Err(ErrorCode::ServerFull);
            }
            let mut salt = [0u8; 16];
            getrandom::getrandom(&mut salt).map_err(|_| ErrorCode::BadRequest)?;
            let hash = krug_core::sha256::hash_password(&salt, password);
            let id = self.next_user;
            self.next_user += 1;
            let record = UserRecord {
                id,
                name: name.to_string(),
                salt,
                hash,
            };
            self.store
                .append_user(&record)
                .map_err(|_| ErrorCode::BadRequest)?;
            self.users_by_name.insert(key, record.clone());
            self.users_by_id.insert(record.id, record.clone());
            record
        };

        if let Some(conn) = self.conns.get_mut(&conn_id) {
            conn.user_id = Some(user.id);
            conn.name = user.name.clone();
        }
        Ok(user.id)
    }

    fn find_direct(&self, a: u64, b: u64) -> Option<u64> {
        self.rooms
            .iter()
            .find(|(_, state)| {
                state.room.kind == ROOM_DM
                    && state.members.contains(&a)
                    && state.members.contains(&b)
            })
            .map(|(id, _)| *id)
    }

    fn open_direct(&mut self, conn_id: u64, target: u64) -> Result<u64, ErrorCode> {
        let Some((me, _)) = self.identity(conn_id) else {
            return Err(ErrorCode::NotAuthed);
        };
        if target == me || !self.users_by_id.contains_key(&target) {
            return Err(ErrorCode::BadRequest);
        }
        let room = match self.find_direct(me, target) {
            Some(room) => room,
            None => {
                let id = self.next_room;
                self.next_room += 1;
                let mut reads = HashMap::new();
                reads.insert(me, 0);
                reads.insert(target, 0);
                self.rooms.insert(
                    id,
                    RoomState {
                        room: Room {
                            id,
                            name: String::new(),
                            topic: String::new(),
                            kind: ROOM_DM,
                            online: 0,
                            peer_id: 0,
                        },
                        messages: VecDeque::new(),
                        next_seq: 1,
                        members: vec![me, target],
                        reads,
                    },
                );
                self.persist_rooms();
                id
            }
        };
        self.join_room(conn_id, room)?;
        self.broadcast_rooms();
        Ok(room)
    }

    fn join_room(&mut self, conn_id: u64, room: u64) -> Result<(), ErrorCode> {
        let Some((uid, _)) = self.identity(conn_id) else {
            return Err(ErrorCode::NotAuthed);
        };
        if !self.is_member(room, uid) {
            return Err(ErrorCode::UnknownRoom);
        }
        let (view, messages, reads) = {
            let state = self.rooms.get(&room).ok_or(ErrorCode::UnknownRoom)?;
            let messages: Vec<Message> = state
                .messages
                .iter()
                .rev()
                .take(50)
                .rev()
                .cloned()
                .map(|mut m| {
                    m.audio.clear();
                    m
                })
                .collect();
            let reads: Vec<ReadState> = state
                .reads
                .iter()
                .map(|(user, seq)| ReadState {
                    user: *user,
                    seq: *seq,
                })
                .collect();
            let view = self
                .room_view(room, uid)
                .ok_or(ErrorCode::UnknownRoom)?;
            (view, messages, reads)
        };
        if let Some(conn) = self.conns.get_mut(&conn_id) {
            conn.rooms.insert(room);
        }
        let members = self.member_users(room);
        let frame = ServerOp::Joined {
            room: view,
            members,
            messages,
            reads,
        }
        .encode();
        self.send_to(conn_id, &frame);
        Ok(())
    }

    fn mark_read(&mut self, room: u64, user: u64, seq: u64) {
        let Some(state) = self.rooms.get_mut(&room) else {
            return;
        };
        if !state.members.contains(&user) {
            return;
        }
        let max = state.next_seq.saturating_sub(1);
        let seq = seq.min(max);
        let cur = state.reads.entry(user).or_insert(0);
        if seq <= *cur {
            return;
        }
        *cur = seq;
        let frame = ServerOp::ReadReceipt { room, user, seq }.encode();
        self.broadcast_participants(room, &frame, None);
    }

    fn history(&self, room: u64, before: u64, limit: usize) -> (Vec<Message>, bool) {
        let Some(state) = self.rooms.get(&room) else {
            return (Vec::new(), true);
        };
        let mut messages: Vec<Message> = state
            .messages
            .iter()
            .filter(|m| before == 0 || m.seq < before)
            .cloned()
            .map(|mut m| {
                m.audio.clear();
                m
            })
            .collect();
        if messages.len() > limit {
            messages.drain(..messages.len() - limit);
        }
        let complete = messages.first().map(|m| m.seq <= 1).unwrap_or(true);
        (messages, complete)
    }

    fn store_message(&mut self, uid: u64, name: &str, room: u64, text: String) -> Option<Message> {
        let state = self.rooms.get_mut(&room)?;
        let seq = state.next_seq;
        state.next_seq += 1;
        let msg = Message {
            room,
            seq,
            ts_ms: now_ms(),
            author_id: uid,
            author: name.to_string(),
            text,
            voice: false,
            sticker: 0,
            audio: Vec::new(),
        };
        state.messages.push_back(msg.clone());
        if state.messages.len() > HISTORY_LIMIT {
            state.messages.pop_front();
        }
        if let Err(e) = self.store.append_message(&msg) {
            eprintln!("[krug] не удалось записать сообщение: {e}");
        }
        Some(msg)
    }

    fn store_sticker(
        &mut self,
        uid: u64,
        name: &str,
        room: u64,
        sticker: u8,
    ) -> Option<Message> {
        let state = self.rooms.get_mut(&room)?;
        let seq = state.next_seq;
        state.next_seq += 1;
        let msg = Message {
            room,
            seq,
            ts_ms: now_ms(),
            author_id: uid,
            author: name.to_string(),
            text: String::new(),
            voice: false,
            sticker,
            audio: Vec::new(),
        };
        state.messages.push_back(msg.clone());
        if state.messages.len() > HISTORY_LIMIT {
            state.messages.pop_front();
        }
        if let Err(e) = self.store.append_message(&msg) {
            eprintln!("[krug] не удалось записать стикер: {e}");
        }
        Some(msg)
    }

    fn set_profile(
        &mut self,
        user: u64,
        handle: &str,
        about: &str,
        presence: u8,
    ) -> Result<(), String> {
        let handle = handle.trim().trim_start_matches('@').to_lowercase();
        if !(3..=20).contains(&handle.chars().count())
            || !handle
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err("Ник: 3–20 символов, латиница, цифры и _".into());
        }
        if let Some(owner) = self.handles.get(&handle) {
            if *owner != user {
                return Err("Этот ник уже занят".into());
            }
        }
        let about = sanitize_text(about);
        let about: String = about.chars().take(80).collect();
        if presence > 3 {
            return Err("Неизвестный статус".into());
        }
        let old = self.profiles.get(&user).cloned();
        if let Some(old) = &old {
            self.handles.remove(&old.handle);
        }
        self.handles.insert(handle.clone(), user);
        self.profiles.insert(
            user,
            Profile {
                handle,
                about,
                presence,
            },
        );
        self.persist_profiles();
        let public = self.public_user(user);
        self.broadcast_users();
        let frame = ServerOp::Profile { user: public }.encode();
        for conn in self.conns.values() {
            if conn.user_id.is_some() {
                let _ = conn.tx.send(frame.clone());
            }
        }
        Ok(())
    }

    fn persist_profiles(&self) {
        let records: Vec<ProfileRecord> = self
            .profiles
            .iter()
            .map(|(id, profile)| ProfileRecord {
                id: *id,
                handle: profile.handle.clone(),
                about: profile.about.clone(),
                presence: profile.presence,
            })
            .collect();
        if let Err(e) = self.store.save_profiles(&records) {
            eprintln!("[krug] не удалось сохранить профили: {e}");
        }
    }

    fn store_voice(
        &mut self,
        uid: u64,
        name: &str,
        room: u64,
        audio: Vec<u8>,
    ) -> Option<Message> {
        let state = self.rooms.get_mut(&room)?;
        let seq = state.next_seq;
        state.next_seq += 1;
        let msg = Message {
            room,
            seq,
            ts_ms: now_ms(),
            author_id: uid,
            author: name.to_string(),
            text: "▶ Голосовое сообщение".to_string(),
            voice: true,
            sticker: 0,
            audio,
        };
        state.messages.push_back(msg.clone());
        if state.messages.len() > HISTORY_LIMIT {
            state.messages.pop_front();
        }
        if let Err(e) = self.store.append_message(&msg) {
            eprintln!("[krug] не удалось записать голосовое: {e}");
        }
        Some(msg)
    }

    fn persist_rooms(&self) {
        let records: Vec<RoomRecord> = self
            .rooms
            .values()
            .map(|state| RoomRecord {
                id: state.room.id,
                name: state.room.name.clone(),
                topic: state.room.topic.clone(),
                kind: state.room.kind,
                members: state.members.clone(),
            })
            .collect();
        if let Err(e) = self.store.save_rooms(&records) {
            eprintln!("[krug] не удалось сохранить список чатов: {e}");
        }
    }

    fn take_message_token(&mut self, conn_id: u64) -> bool {
        let Some(conn) = self.conns.get_mut(&conn_id) else {
            return false;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(conn.last_refill).as_secs_f32();
        conn.last_refill = now;
        conn.tokens = (conn.tokens + elapsed * 3.0).min(6.0);
        if conn.tokens >= 1.0 {
            conn.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn take_typing_token(&mut self, conn_id: u64) -> bool {
        let Some(conn) = self.conns.get_mut(&conn_id) else {
            return false;
        };
        let now = Instant::now();
        let ok = conn
            .last_typing
            .map(|t| now.duration_since(t) >= Duration::from_millis(1500))
            .unwrap_or(true);
        if ok {
            conn.last_typing = Some(now);
        }
        ok
    }
}
