//! Чтение и запись примитивов протокола: всё little-endian, строки с
//! двухбайтовой длиной. Никакой рефлексии, serde и прочих зависимостей.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Ошибка разбора кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    /// Кадр короче, чем нужно.
    Truncated,
    /// Строка не является корректным UTF-8.
    BadUtf8,
    /// Поле вышло за разрешённый размер.
    TooLarge,
    /// Неизвестный код операции.
    BadOpcode(u8),
    /// Неизвестная версия протокола.
    BadVersion(u16),
    /// Значение вне допустимого диапазона.
    BadValue,
}

impl core::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProtoError::Truncated => write!(f, "кадр обрезан"),
            ProtoError::BadUtf8 => write!(f, "некорректный UTF-8"),
            ProtoError::TooLarge => write!(f, "поле слишком большое"),
            ProtoError::BadOpcode(op) => write!(f, "неизвестная операция 0x{op:02x}"),
            ProtoError::BadVersion(v) => write!(f, "несовместимая версия {v}"),
            ProtoError::BadValue => write!(f, "недопустимое значение"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ProtoError {}

pub type Result<T> = core::result::Result<T, ProtoError>;

/// Пишущая сторона кадра.
#[derive(Debug)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Новый кадр с кодом операции.
    pub fn new(op: u8) -> Writer {
        let mut buf = Vec::with_capacity(64);
        buf.push(op);
        Writer { buf }
    }

    /// Пустой кадр — когда код операции дописывается вручную.
    pub fn bare() -> Writer {
        Writer {
            buf: Vec::with_capacity(64),
        }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn str(&mut self, s: &str) -> &mut Self {
        // Длины заранее ограничены вызывающим кодом; здесь только страховка.
        let n = s.len().min(u16::MAX as usize);
        self.u16(n as u16);
        self.buf.extend_from_slice(&s.as_bytes()[..n]);
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Читающая сторона кадра.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(ProtoError::TooLarge)?;
        if end > self.data.len() {
            return Err(ProtoError::Truncated);
        }
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_le_bytes(arr))
    }

    pub fn str(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let b = self.take(n)?;
        core::str::from_utf8(b)
            .map(|s| s.to_string())
            .map_err(|_| ProtoError::BadUtf8)
    }

    /// Строка с ограничением длины (проверяется после чтения).
    pub fn limited_str(&mut self, max: usize) -> Result<String> {
        let s = self.str()?;
        if s.len() > max {
            return Err(ProtoError::TooLarge);
        }
        Ok(s)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_done(&self) -> bool {
        self.pos >= self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut w = Writer::new(7);
        w.u8(1).u16(2).u32(3).u64(4).str("привет").bool(true);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16().unwrap(), 2);
        assert_eq!(r.u32().unwrap(), 3);
        assert_eq!(r.u64().unwrap(), 4);
        assert_eq!(r.str().unwrap(), "привет");
        assert!(r.bool().unwrap());
        assert!(r.is_done());
    }

    #[test]
    fn truncated() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.u32(), Err(ProtoError::Truncated));
    }

    #[test]
    fn limits() {
        let mut w = Writer::new(0);
        w.str("abcd");
        let bytes = w.finish();
        let mut r = Reader::new(&bytes[1..]);
        assert_eq!(r.limited_str(3), Err(ProtoError::TooLarge));
    }
}
