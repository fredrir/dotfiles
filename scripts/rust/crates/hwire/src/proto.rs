//! What the two halves say to each other.
//!
//! One connection carries one phase: the client dials, writes a fixed 32-byte
//! header naming the phase, and both sides then do that phase's work until the
//! sender shuts its half down. Nothing is multiplexed, so a phase needs no
//! framing beyond its header, and parallel streams are just several
//! connections carrying the same phase at the same time.
//!
//! The token is the whole of the access control. A server is started for one
//! measurement, listens on one route address, and is handed a fresh token over
//! ssh; anything that dials it without repeating that token gets its
//! connection closed. This keeps a stray or overlapping run from being
//! measured as part of this one — it is not protecting a secret, since the
//! only thing on the connection after it is a stream of zeros.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

pub const MAGIC: [u8; 4] = *b"HWIR";
pub const VERSION: u8 = 1;
pub const HEADER: usize = 32;
pub const RESULT: usize = 16;

/// The payload bounced back and forth in the latency phase: a sequence
/// number, so a reply that belongs to an earlier round trip is caught.
pub const PING: usize = 8;

/// One write's worth of zeros. Large enough that the per-call cost is noise
/// at cable speed, small enough to stay well inside a socket buffer.
pub const CHUNK: usize = 256 * 1024;

/// Time the receiver throws away before it starts counting, and the sender
/// adds to what it was asked for. TCP opens a connection with a small
/// congestion window and doubles it once per round trip, so the first
/// milliseconds of any transfer measure the ramp rather than the link;
/// discarding them is what `iperf3 -O` does by hand.
pub const WARMUP: Duration = Duration::from_millis(150);

/// How long either side waits on a peer that has stopped saying anything.
pub const STALL: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Bounce small payloads back to the client until it stops.
    Ping,
    /// The client sends, the server counts what arrives.
    Send,
    /// The server sends, the client counts what arrives.
    Recv,
    /// Nothing to measure: the server is done and should exit.
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
    /// The measurement window the receiver should end up reporting.
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

    /// `Err` describes what is wrong with the header for the log; the server
    /// answers every one of them the same way, by hanging up.
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

/// What a receiver counted, in its own time. Whoever received reports it, so
/// throughput is always the rate the bytes actually landed at rather than the
/// rate they were handed to the kernel at.
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

/// Send zeros for `window` plus the warmup, then shut the sending half down,
/// which is the end of the transfer the other side is timing.
pub fn blast(stream: &mut TcpStream, window: Duration) -> io::Result<()> {
    let zeros = [0u8; CHUNK];
    let until = Instant::now() + window + WARMUP;
    while Instant::now() < until {
        stream.write_all(&zeros)?;
    }
    stream.shutdown(Shutdown::Write)
}

/// Read until the sender is done, counting only what arrives after the
/// warmup. Timing starts at the first byte counted and ends at the last, so
/// the answer describes the transfer and not the connection setup around it.
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

/// A fresh token for one measurement.
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
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            mode: Mode::Recv,
            token: [7u8; 16],
            window: Duration::from_millis(1500),
        }
    }

    #[test]
    fn a_header_survives_the_round_trip() {
        let decoded = Header::decode(&header().encode()).unwrap();
        assert_eq!(decoded.mode, Mode::Recv);
        assert_eq!(decoded.token, [7u8; 16]);
        assert_eq!(decoded.window, Duration::from_millis(1500));
    }

    #[test]
    fn every_phase_has_a_code_of_its_own() {
        for mode in [Mode::Ping, Mode::Send, Mode::Recv, Mode::Bye] {
            assert_eq!(Mode::from_code(mode.code()), Some(mode));
        }
        assert_eq!(Mode::from_code(0), None);
        assert_eq!(Mode::from_code(5), None);
    }

    #[test]
    fn a_foreign_connection_is_told_apart_from_an_old_build() {
        let mut bytes = header().encode();
        bytes[..4].copy_from_slice(b"HTTP");
        assert!(Header::decode(&bytes).unwrap_err().contains("is not hwire"));

        let mut bytes = header().encode();
        bytes[4] = VERSION + 1;
        assert!(
            Header::decode(&bytes)
                .unwrap_err()
                .contains("different builds")
        );
    }

    #[test]
    fn a_count_survives_the_round_trip() {
        let counted = Counted {
            bytes: 4_500_000_000,
            elapsed: Duration::from_millis(1000),
        };
        let decoded = Counted::decode(&counted.encode());
        assert_eq!(decoded.bytes, counted.bytes);
        assert_eq!(decoded.elapsed, counted.elapsed);
        assert_eq!(decoded.bits_per_second(), 36_000_000_000.0);
    }

    #[test]
    fn a_count_with_no_time_in_it_is_not_a_division_by_zero() {
        assert_eq!(Counted::default().bits_per_second(), 0.0);
    }

    #[test]
    fn tokens_are_hex_both_ways() {
        let token = token().unwrap();
        assert_eq!(unhex(&hex(&token)).unwrap(), token);
        assert_eq!(hex(&[0u8; 16]).len(), 32);
        assert!(unhex("abc").is_err());
        assert!(unhex(&"z".repeat(32)).is_err());
    }

    #[test]
    fn two_tokens_are_not_the_same_token() {
        assert_ne!(token().unwrap(), token().unwrap());
    }
}
