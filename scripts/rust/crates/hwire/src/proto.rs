use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

pub const MAGIC: [u8; 4] = *b"HWIR";
pub const VERSION: u8 = 1;
pub const HEADER: usize = 32;
pub const RESULT: usize = 16;

pub const PING: usize = 8;

pub const CHUNK: usize = 256 * 1024;

pub const WARMUP: Duration = Duration::from_millis(150);

pub const STALL: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Ping,
    Send,
    Recv,
    Bye,
}

impl Mode {
    fn code(self) -> u8 {
        match self {
            Mode::Ping => 1,
            Mode::Send => 2,
            Mode::Recv => 3,
            Mode::Bye => 4,
        }
    }

    fn from_code(code: u8) -> Option<Mode> {
        match code {
            1 => Some(Mode::Ping),
            2 => Some(Mode::Send),
            3 => Some(Mode::Recv),
            4 => Some(Mode::Bye),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub mode: Mode,
    pub token: [u8; 16],
    pub window: Duration,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER] {
        let mut bytes = [0u8; HEADER];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4] = VERSION;
        bytes[5] = self.mode.code();
        bytes[8..24].copy_from_slice(&self.token);
        let millis = self.window.as_millis().min(u32::MAX as u128) as u32;
        bytes[24..28].copy_from_slice(&millis.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; HEADER]) -> Result<Header, String> {
        if bytes[..4] != MAGIC {
            return Err("the other end of this connection is not hwire".into());
        }
        if bytes[4] != VERSION {
            return Err(format!(
                "protocol version {}, expected {VERSION}: the two machines are running different builds",
                bytes[4]
            ));
        }
        let Some(mode) = Mode::from_code(bytes[5]) else {
            return Err(format!("unknown phase {}", bytes[5]));
        };
        let mut token = [0u8; 16];
        token.copy_from_slice(&bytes[8..24]);
        let millis = u32::from_be_bytes(bytes[24..28].try_into().expect("four bytes"));
        Ok(Header {
            mode,
            token,
            window: Duration::from_millis(millis as u64),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counted {
    pub bytes: u64,
    pub elapsed: Duration,
}

impl Counted {
    pub fn encode(&self) -> [u8; RESULT] {
        let mut bytes = [0u8; RESULT];
        bytes[..8].copy_from_slice(&self.bytes.to_be_bytes());
        let nanos = self.elapsed.as_nanos().min(u64::MAX as u128) as u64;
        bytes[8..].copy_from_slice(&nanos.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8; RESULT]) -> Counted {
        Counted {
            bytes: u64::from_be_bytes(bytes[..8].try_into().expect("eight bytes")),
            elapsed: Duration::from_nanos(u64::from_be_bytes(
                bytes[8..].try_into().expect("eight bytes"),
            )),
        }
    }

    pub fn bits_per_second(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.bytes as f64 * 8.0 / self.elapsed.as_secs_f64()
    }
}

pub fn blast(stream: &mut TcpStream, window: Duration) -> io::Result<()> {
    let zeros = [0u8; CHUNK];
    let until = Instant::now() + window + WARMUP;
    while Instant::now() < until {
        stream.write_all(&zeros)?;
    }
    stream.shutdown(Shutdown::Write)
}

pub fn drain(stream: &mut TcpStream) -> io::Result<Counted> {
    let mut buffer = vec![0u8; CHUNK];
    let mut counted = Counted::default();
    let mut first: Option<Instant> = None;
    let mut start: Option<Instant> = None;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(counted);
        }
        let now = Instant::now();
        let first = *first.get_or_insert(now);
        match start {
            None if now.duration_since(first) >= WARMUP => start = Some(now),
            None => continue,
            Some(start) => {
                counted.bytes += read as u64;
                counted.elapsed = now.duration_since(start);
            }
        }
    }
}

pub fn read_exactly<const N: usize>(stream: &mut TcpStream) -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub fn token() -> io::Result<[u8; 16]> {
    let mut token = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut token)?;
    Ok(token)
}

pub fn hex(token: &[u8; 16]) -> String {
    token.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn unhex(text: &str) -> Result<[u8; 16], String> {
    let digits: Vec<u8> = text.bytes().collect();
    if digits.len() != 32 {
        return Err("a token is 32 hex digits".into());
    }
    let mut token = [0u8; 16];
    for (byte, pair) in token.iter_mut().zip(digits.chunks_exact(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| "a token is 32 hex digits".to_string())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| "a token is 32 hex digits".to_string())?;
    }
    Ok(token)
}

#[cfg(test)]
#[path = "../tests/unit/proto_tests.rs"]
mod tests;
