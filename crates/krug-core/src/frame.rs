//! Кадрирование потока: `u32` длина + полезная нагрузка.
//!
//! Доступно только со `std` — на консолях транспорт свой.

use std::io::{self, Read, Write};

use crate::MAX_FRAME;

/// Пишет один кадр и сбрасывает буфер.
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "недопустимый размер кадра",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Читает один кадр. `Ok(None)` — чистое закрытие соединения.
pub fn read_frame<R: Read>(reader: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "битая длина кадра",
        ));
    }
    let mut buf = vec![0u8; n];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        write_frame(&mut buf, b"world").unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur, 1024).unwrap().unwrap(), b"hello");
        assert_eq!(read_frame(&mut cur, 1024).unwrap().unwrap(), b"world");
        assert!(read_frame(&mut cur, 1024).unwrap().is_none());
    }

    #[test]
    fn rejects_huge() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_FRAME as u32 + 1).to_le_bytes());
        let mut cur = Cursor::new(buf);
        assert!(read_frame(&mut cur, MAX_FRAME).is_err());
    }
}
