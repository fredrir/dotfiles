use std::io::{self, Read, Write};

pub const VERSION: u8 = 1;
pub const PASTE: u8 = b'p';
pub const REQUEST: [u8; 2] = [VERSION, PASTE];
pub const HEADER: usize = 6;
pub const LIMIT: usize = 64 * 1024 * 1024;

const OK: u8 = 0;
const FAILED: u8 = 1;

pub fn check_request(bytes: [u8; 2]) -> Result<(), String> {
    match bytes {
        [VERSION, PASTE] => Ok(()),
        [VERSION, op] => Err(format!("unknown request {op:#04x}")),
        [version, _] => Err(format!("unsupported version {version}")),
    }
}

pub fn write_reply(writer: &mut impl Write, reply: &Result<String, String>) -> io::Result<()> {
    let (status, body) = match reply {
        Ok(text) => (OK, text.as_bytes()),
        Err(message) => (FAILED, message.as_bytes()),
    };
    let length = u32::try_from(body.len()).map_err(io::Error::other)?;
    let mut header = [VERSION, status, 0, 0, 0, 0];
    header[2..].copy_from_slice(&length.to_be_bytes());
    writer.write_all(&header)?;
    writer.write_all(body)?;
    writer.flush()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub ok: bool,
    pub length: usize,
}

pub fn parse_header(bytes: [u8; HEADER]) -> Result<Header, String> {
    if bytes[0] != VERSION {
        return Err(format!("unsupported version {}", bytes[0]));
    }
    let ok = match bytes[1] {
        OK => true,
        FAILED => false,
        status => return Err(format!("unknown status {status}")),
    };
    let length = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize;
    if length > LIMIT {
        return Err("reply too large".into());
    }
    Ok(Header { ok, length })
}

pub fn read_body(reader: &mut impl Read, header: &Header) -> Result<String, String> {
    let mut body = Vec::with_capacity(header.length);
    reader
        .take(header.length as u64)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() != header.length {
        return Err("reply truncated".into());
    }
    let text = String::from_utf8(body).map_err(|_| "reply is not UTF-8".to_string())?;
    if header.ok { Ok(text) } else { Err(text) }
}

#[cfg(test)]
#[path = "../tests/unit/proto_tests.rs"]
mod tests;
