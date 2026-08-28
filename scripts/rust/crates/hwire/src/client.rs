use std::io::{self, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

use crate::proto::{self, Counted, Header, Mode};
use hostkit::socket;

const CONNECT: Duration = Duration::from_secs(5);

const WARMUP_PINGS: usize = 5;

const ENOUGH_PINGS: usize = 10;

#[derive(Clone, Copy)]
pub struct Peer {
    pub address: SocketAddrV4,
    pub local: Option<Ipv4Addr>,
    pub token: [u8; 16],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Up,
    Down,
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

impl Peer {
    fn dial(&self, mode: Mode, window: Duration) -> io::Result<TcpStream> {
        let mut stream = socket::connect(self.local, self.address, CONNECT)?;
        stream.set_read_timeout(Some(proto::STALL))?;
        stream.set_write_timeout(Some(proto::STALL))?;
        let header = Header {
            mode,
            token: self.token,
            window,
        };
        stream.write_all(&header.encode())?;
        Ok(stream)
    }

    pub fn latency(&self, samples: usize, budget: Duration) -> io::Result<Latency> {
        let mut stream = self.dial(Mode::Ping, Duration::ZERO)?;
        stream.set_nodelay(true)?;
        let mut round_trips = Vec::with_capacity(samples.min(1024));
        let phase = Instant::now();
        for sequence in 0..(WARMUP_PINGS + samples) as u64 {
            let sent = sequence.to_be_bytes();
            let started = Instant::now();
            stream.write_all(&sent)?;
            let echoed = proto::read_exactly::<{ proto::PING }>(&mut stream)?;
            let elapsed = started.elapsed();
            if echoed != sent {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the peer answered a different round trip",
                ));
            }
            if sequence as usize >= WARMUP_PINGS {
                round_trips.push(elapsed);
                if round_trips.len() >= ENOUGH_PINGS && phase.elapsed() >= budget {
                    break;
                }
            }
        }
        stream.shutdown(Shutdown::Write)?;
        round_trips.sort_unstable();
        Ok(Latency { round_trips })
    }

    pub fn transfer(
        &self,
        direction: Direction,
        window: Duration,
        streams: usize,
    ) -> io::Result<Counted> {
        let counted = std::thread::scope(|scope| {
            let running: Vec<_> = (0..streams)
                .map(|_| scope.spawn(move || self.stream(direction, window)))
                .collect();
            running
                .into_iter()
                .map(|stream| {
                    stream.join().unwrap_or_else(|_| {
                        Err(io::Error::other("a transfer thread stopped unexpectedly"))
                    })
                })
                .collect::<io::Result<Vec<Counted>>>()
        })?;
        Ok(combine(&counted))
    }

    fn stream(&self, direction: Direction, window: Duration) -> io::Result<Counted> {
        match direction {
            Direction::Up => {
                let mut stream = self.dial(Mode::Send, window)?;
                proto::blast(&mut stream, window)?;
                Ok(Counted::decode(&proto::read_exactly::<{ proto::RESULT }>(
                    &mut stream,
                )?))
            }
            Direction::Down => {
                let mut stream = self.dial(Mode::Recv, window)?;
                proto::drain(&mut stream)
            }
        }
    }

    pub fn bye(&self) {
        let _ = self.dial(Mode::Bye, Duration::ZERO);
    }
}

fn combine(counted: &[Counted]) -> Counted {
    Counted {
        bytes: counted.iter().map(|counted| counted.bytes).sum(),
        elapsed: counted
            .iter()
            .map(|counted| counted.elapsed)
            .max()
            .unwrap_or_default(),
    }
}

pub struct Latency {
    round_trips: Vec<Duration>,
}

impl Latency {
    pub fn samples(&self) -> usize {
        self.round_trips.len()
    }

    pub fn min(&self) -> Duration {
        self.round_trips.first().copied().unwrap_or_default()
    }

    pub fn max(&self) -> Duration {
        self.round_trips.last().copied().unwrap_or_default()
    }

    pub fn mean(&self) -> Duration {
        if self.round_trips.is_empty() {
            return Duration::ZERO;
        }
        self.round_trips.iter().sum::<Duration>() / self.round_trips.len() as u32
    }

    pub fn percentile(&self, percent: f64) -> Duration {
        if self.round_trips.is_empty() {
            return Duration::ZERO;
        }
        let rank = (percent / 100.0 * self.round_trips.len() as f64).ceil() as usize;
        self.round_trips[rank.clamp(1, self.round_trips.len()) - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latency(millis: &[u64]) -> Latency {
        let mut round_trips: Vec<Duration> =
            millis.iter().map(|&ms| Duration::from_millis(ms)).collect();
        round_trips.sort_unstable();
        Latency { round_trips }
    }

    #[test]
    fn the_statistics_describe_the_samples() {
        let latency = latency(&[4, 1, 2, 3]);
        assert_eq!(latency.samples(), 4);
        assert_eq!(latency.min(), Duration::from_millis(1));
        assert_eq!(latency.max(), Duration::from_millis(4));
        assert_eq!(
            latency.mean(),
            Duration::from_millis(2) + Duration::from_micros(500)
        );
        assert_eq!(latency.percentile(50.0), Duration::from_millis(2));
        assert_eq!(latency.percentile(99.0), Duration::from_millis(4));
    }

    #[test]
    fn a_percentile_of_one_sample_is_that_sample() {
        let latency = latency(&[7]);
        assert_eq!(latency.percentile(0.0), Duration::from_millis(7));
        assert_eq!(latency.percentile(50.0), Duration::from_millis(7));
        assert_eq!(latency.percentile(100.0), Duration::from_millis(7));
    }

    #[test]
    fn no_samples_is_zero_rather_than_a_panic() {
        let latency = latency(&[]);
        assert_eq!(latency.min(), Duration::ZERO);
        assert_eq!(latency.max(), Duration::ZERO);
        assert_eq!(latency.mean(), Duration::ZERO);
        assert_eq!(latency.percentile(50.0), Duration::ZERO);
    }

    #[test]
    fn parallel_streams_add_their_bytes_and_share_their_time() {
        let combined = combine(&[
            Counted {
                bytes: 1_000,
                elapsed: Duration::from_millis(900),
            },
            Counted {
                bytes: 3_000,
                elapsed: Duration::from_millis(1_000),
            },
        ]);
        assert_eq!(combined.bytes, 4_000);
        assert_eq!(combined.elapsed, Duration::from_millis(1_000));
    }

    #[test]
    fn combining_nothing_is_not_a_division_by_zero() {
        assert_eq!(combine(&[]).bits_per_second(), 0.0);
    }
}
