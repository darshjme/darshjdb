// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache-server :: codec — RESP3 frame encoder/decoder.
//
// Implements enough of RESP3 (RESP2 is a strict subset) to dispatch every
// command listed in Slice 11. Partial frames yield Ok(None) so Framed waits
// for more bytes.

use std::io;

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// Every variant listed in Slice 11 Part A, item 3.
#[derive(Debug, Clone, PartialEq)]
pub enum RespFrame {
    SimpleString(String),
    Error(String),
    Integer(i64),
    BulkString(Option<Vec<u8>>),
    Array(Option<Vec<RespFrame>>),
    Null,
    Map(Vec<(RespFrame, RespFrame)>),
    Set(Vec<RespFrame>),
    Double(f64),
    Boolean(bool),
}

impl RespFrame {
    pub fn ok() -> Self {
        RespFrame::SimpleString("OK".into())
    }

    pub fn err(msg: impl Into<String>) -> Self {
        RespFrame::Error(msg.into())
    }

    pub fn bulk(bytes: impl Into<Vec<u8>>) -> Self {
        RespFrame::BulkString(Some(bytes.into()))
    }

    pub fn nil_bulk() -> Self {
        RespFrame::BulkString(None)
    }

    pub fn as_string(&self) -> Option<String> {
        match self {
            RespFrame::SimpleString(s) => Some(s.clone()),
            RespFrame::BulkString(Some(b)) => std::str::from_utf8(b).ok().map(|s| s.to_string()),
            RespFrame::Integer(i) => Some(i.to_string()),
            RespFrame::Double(d) => Some(d.to_string()),
            _ => None,
        }
    }
}

/// Codec implementing RESP3 framing over `BytesMut`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RESP3Codec;

impl Encoder<RespFrame> for RESP3Codec {
    type Error = io::Error;

    fn encode(&mut self, item: RespFrame, dst: &mut BytesMut) -> Result<(), Self::Error> {
        write_frame(&item, dst);
        Ok(())
    }
}

impl Decoder for RESP3Codec {
    type Item = RespFrame;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match parse_frame(src)? {
            Some((frame, consumed)) => {
                src.advance(consumed);
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

fn write_frame(frame: &RespFrame, dst: &mut BytesMut) {
    match frame {
        RespFrame::SimpleString(s) => {
            dst.put_u8(b'+');
            dst.extend_from_slice(s.as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
        RespFrame::Error(e) => {
            dst.put_u8(b'-');
            dst.extend_from_slice(e.as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
        RespFrame::Integer(i) => {
            dst.put_u8(b':');
            dst.extend_from_slice(i.to_string().as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
        RespFrame::BulkString(Some(bytes)) => {
            dst.put_u8(b'$');
            dst.extend_from_slice(bytes.len().to_string().as_bytes());
            dst.extend_from_slice(b"\r\n");
            dst.extend_from_slice(bytes);
            dst.extend_from_slice(b"\r\n");
        }
        RespFrame::BulkString(None) => {
            dst.extend_from_slice(b"$-1\r\n");
        }
        RespFrame::Array(Some(items)) => {
            dst.put_u8(b'*');
            dst.extend_from_slice(items.len().to_string().as_bytes());
            dst.extend_from_slice(b"\r\n");
            for item in items {
                write_frame(item, dst);
            }
        }
        RespFrame::Array(None) => {
            dst.extend_from_slice(b"*-1\r\n");
        }
        RespFrame::Null => {
            dst.extend_from_slice(b"_\r\n");
        }
        RespFrame::Map(pairs) => {
            dst.put_u8(b'%');
            dst.extend_from_slice(pairs.len().to_string().as_bytes());
            dst.extend_from_slice(b"\r\n");
            for (k, v) in pairs {
                write_frame(k, dst);
                write_frame(v, dst);
            }
        }
        RespFrame::Set(items) => {
            dst.put_u8(b'~');
            dst.extend_from_slice(items.len().to_string().as_bytes());
            dst.extend_from_slice(b"\r\n");
            for item in items {
                write_frame(item, dst);
            }
        }
        RespFrame::Double(d) => {
            dst.put_u8(b',');
            dst.extend_from_slice(format!("{d}").as_bytes());
            dst.extend_from_slice(b"\r\n");
        }
        RespFrame::Boolean(b) => {
            dst.extend_from_slice(if *b { b"#t\r\n" } else { b"#f\r\n" });
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn parse_frame(src: &[u8]) -> io::Result<Option<(RespFrame, usize)>> {
    if src.is_empty() {
        return Ok(None);
    }
    let tag = src[0];
    let body = &src[1..];
    match tag {
        b'+' => parse_line(body).map(|r| r.map(|(s, n)| (RespFrame::SimpleString(s), n + 1))),
        b'-' => parse_line(body).map(|r| r.map(|(s, n)| (RespFrame::Error(s), n + 1))),
        b':' => match parse_line(body)? {
            Some((s, n)) => {
                let v: i64 = s
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad integer"))?;
                Ok(Some((RespFrame::Integer(v), n + 1)))
            }
            None => Ok(None),
        },
        b',' => match parse_line(body)? {
            Some((s, n)) => {
                let v: f64 = s
                    .parse()
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad double"))?;
                Ok(Some((RespFrame::Double(v), n + 1)))
            }
            None => Ok(None),
        },
        b'#' => match parse_line(body)? {
            Some((s, n)) => {
                let v = match s.as_str() {
                    "t" => true,
                    "f" => false,
                    _ => {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad boolean"));
                    }
                };
                Ok(Some((RespFrame::Boolean(v), n + 1)))
            }
            None => Ok(None),
        },
        b'_' => match parse_line(body)? {
            Some((_, n)) => Ok(Some((RespFrame::Null, n + 1))),
            None => Ok(None),
        },
        b'$' => parse_bulk(body).map(|r| r.map(|(f, n)| (f, n + 1))),
        b'*' => parse_array(body, b'*').map(|r| r.map(|(f, n)| (f, n + 1))),
        b'~' => parse_array(body, b'~').map(|r| r.map(|(f, n)| (f, n + 1))),
        b'%' => parse_map(body).map(|r| r.map(|(f, n)| (f, n + 1))),
        // Inline commands ("PING\r\n" without a RESP tag).
        _ => parse_inline(src),
    }
}

fn parse_line(src: &[u8]) -> io::Result<Option<(String, usize)>> {
    for i in 0..src.len().saturating_sub(1) {
        if src[i] == b'\r' && src[i + 1] == b'\n' {
            let s = std::str::from_utf8(&src[..i])
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad utf8"))?
                .to_string();
            return Ok(Some((s, i + 2)));
        }
    }
    Ok(None)
}

fn parse_bulk(src: &[u8]) -> io::Result<Option<(RespFrame, usize)>> {
    let Some((len_str, header_n)) = parse_line(src)? else {
        return Ok(None);
    };
    let len: i64 = len_str
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad bulk length"))?;
    if len < 0 {
        return Ok(Some((RespFrame::BulkString(None), header_n)));
    }
    let len = len as usize;
    if src.len() < header_n + len + 2 {
        return Ok(None);
    }
    let data = src[header_n..header_n + len].to_vec();
    if &src[header_n + len..header_n + len + 2] != b"\r\n" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "missing bulk CRLF"));
    }
    Ok(Some((RespFrame::BulkString(Some(data)), header_n + len + 2)))
}

fn parse_array(src: &[u8], kind: u8) -> io::Result<Option<(RespFrame, usize)>> {
    let Some((len_str, header_n)) = parse_line(src)? else {
        return Ok(None);
    };
    let len: i64 = len_str
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad array length"))?;
    if len < 0 {
        return Ok(Some((RespFrame::Array(None), header_n)));
    }
    let len = len as usize;
    let mut items = Vec::with_capacity(len);
    let mut cursor = header_n;
    for _ in 0..len {
        match parse_frame(&src[cursor..])? {
            Some((frame, consumed)) => {
                items.push(frame);
                cursor += consumed;
            }
            None => return Ok(None),
        }
    }
    let frame = if kind == b'~' { RespFrame::Set(items) } else { RespFrame::Array(Some(items)) };
    Ok(Some((frame, cursor)))
}

fn parse_map(src: &[u8]) -> io::Result<Option<(RespFrame, usize)>> {
    let Some((len_str, header_n)) = parse_line(src)? else {
        return Ok(None);
    };
    let len: i64 = len_str
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad map length"))?;
    if len < 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "map cannot be null"));
    }
    let len = len as usize;
    let mut pairs = Vec::with_capacity(len);
    let mut cursor = header_n;
    for _ in 0..len {
        let Some((k, kn)) = parse_frame(&src[cursor..])? else { return Ok(None) };
        cursor += kn;
        let Some((v, vn)) = parse_frame(&src[cursor..])? else { return Ok(None) };
        cursor += vn;
        pairs.push((k, v));
    }
    Ok(Some((RespFrame::Map(pairs), cursor)))
}

fn parse_inline(src: &[u8]) -> io::Result<Option<(RespFrame, usize)>> {
    let Some((line, n)) = parse_line(src)? else {
        return Ok(None);
    };
    let items: Vec<RespFrame> = line
        .split_whitespace()
        .map(|s| RespFrame::BulkString(Some(s.as_bytes().to_vec())))
        .collect();
    Ok(Some((RespFrame::Array(Some(items)), n)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_simple_string() {
        let mut buf = BytesMut::new();
        let mut c = RESP3Codec;
        c.encode(RespFrame::ok(), &mut buf).unwrap();
        assert_eq!(&buf[..], b"+OK\r\n");
    }

    #[test]
    fn encode_bulk_string() {
        let mut buf = BytesMut::new();
        let mut c = RESP3Codec;
        c.encode(RespFrame::bulk(b"PONG".to_vec()), &mut buf).unwrap();
        assert_eq!(&buf[..], b"$4\r\nPONG\r\n");
    }

    #[test]
    fn decode_array_ping() {
        let mut buf = BytesMut::from(&b"*1\r\n$4\r\nPING\r\n"[..]);
        let mut c = RESP3Codec;
        let frame = c.decode(&mut buf).unwrap().unwrap();
        match frame {
            RespFrame::Array(Some(items)) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].as_string().as_deref(), Some("PING"));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn decode_inline_ping() {
        let mut buf = BytesMut::from(&b"PING\r\n"[..]);
        let mut c = RESP3Codec;
        let frame = c.decode(&mut buf).unwrap().unwrap();
        match frame {
            RespFrame::Array(Some(items)) => {
                assert_eq!(items[0].as_string().as_deref(), Some("PING"));
            }
            _ => panic!("expected inline array"),
        }
    }

    #[test]
    fn decode_partial_returns_none() {
        let mut buf = BytesMut::from(&b"*1\r\n$4\r\nPI"[..]);
        let mut c = RESP3Codec;
        assert!(c.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn roundtrip_map_and_double() {
        let mut buf = BytesMut::new();
        let mut c = RESP3Codec;
        let frame = RespFrame::Map(vec![(
            RespFrame::SimpleString("pi".into()),
            RespFrame::Double(3.14),
        )]);
        c.encode(frame.clone(), &mut buf).unwrap();
        let decoded = c.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, frame);
    }
}
