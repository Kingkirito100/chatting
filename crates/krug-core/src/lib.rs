//! Ядро «Круга»: протокол, модель данных и криптография.
//!
//! Крейт не зависит от операционной системы: он собирается и в `no_std`
//! (с `alloc`), поэтому один и тот же код разговора с сервером можно
//! переиспользовать на консоли, в терминале и в оконном клиенте.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod frame;
pub mod model;
pub mod proto;
pub mod sha256;
pub mod wire;

pub use model::{Message, Room, User};
pub use proto::{ClientKind, ClientOp, ErrorCode, ServerOp};
pub use wire::{ProtoError, Reader, Writer};

/// Версия протокола. Несовпадение — вежливый отказ при входе.
pub const PROTOCOL_VERSION: u16 = 2;

/// Максимальный размер одного кадра вместе с заголовком длины.
pub const MAX_FRAME: usize = 64 * 1024;
/// Максимальная длина сообщения в байтах UTF-8.
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
/// Максимальная длина ника.
pub const MAX_NAME_BYTES: usize = 48;
/// Максимальная длина пароля.
pub const MAX_PASSWORD_BYTES: usize = 64;
/// Минимальная длина пароля при регистрации.
pub const MIN_PASSWORD_BYTES: usize = 4;
/// Максимальная длина инвайт-кода.
pub const MAX_INVITE_BYTES: usize = 64;
/// Максимальная длина названия комнаты.
pub const MAX_ROOM_NAME_BYTES: usize = 48;
/// Максимальная длина описания комнаты.
pub const MAX_TOPIC_BYTES: usize = 160;
/// Сколько комнат отдаётся клиенту в одном списке.
pub const MAX_ROOMS: usize = 64;
/// Сколько участников помещается в снимок присутствия.
pub const MAX_MEMBERS: usize = 256;
/// Сколько сообщений можно запросить одним куском истории.
pub const MAX_BATCH: usize = 100;
/// Номера стикеров: 1..=MAX_STICKER. Ноль означает «стикера нет».
pub const MAX_STICKER: u8 = 6;

/// Ник: 1–32 символа, буквы/цифры/пробел/`_-.`, без управляющих символов.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return false;
    }
    if name.chars().count() > 32 || name.starts_with('.') {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
}

/// Пароль: 4–64 байта, без управляющих символов.
pub fn valid_password(password: &str) -> bool {
    (MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&password.len())
        && !password.chars().any(|c| c.is_control())
}

/// Название комнаты: как ник, но без пробелов по краям и без решётки.
pub fn valid_room_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_ROOM_NAME_BYTES {
        return false;
    }
    if name.chars().count() > 32 || name.starts_with('#') || name.starts_with('.') {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
}

/// Убирает управляющие символы (кроме перевода строки) и нормализует пробелы.
pub fn sanitize_text(text: &str) -> alloc::string::String {
    let mut out = alloc::string::String::with_capacity(text.len());
    let mut last_was_space = false;
    for c in text.chars() {
        let c = if c == '\t' { ' ' } else { c };
        if c.is_control() && c != '\n' {
            continue;
        }
        if c == '\n' {
            last_was_space = false;
            out.push(c);
        } else if c == ' ' {
            if !last_was_space {
                out.push(c);
            }
            last_was_space = true;
        } else {
            last_was_space = false;
            out.push(c);
        }
    }
    out.trim().into()
}

/// Сравнение строк за постоянное время — чтобы инвайт нельзя было подобрать
/// по времени ответа.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff = (a.len() ^ b.len()) as u8;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("Аня"));
        assert!(valid_name("player_1"));
        assert!(!valid_name(""));
        assert!(!valid_name("  "));
        assert!(!valid_name("плохое\nимя"));
        assert!(!valid_name(".скрытое"));
    }

    #[test]
    fn rooms() {
        assert!(valid_room_name("general"));
        assert!(valid_room_name("поиграть"));
        assert!(!valid_room_name("#general"));
        assert!(!valid_room_name(""));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_text("  привет   мир \n\n "), "привет мир");
        assert_eq!(sanitize_text("a\tb"), "a b");
        assert_eq!(sanitize_text("x\u{0}y"), "xy");
    }
}
