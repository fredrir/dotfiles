//! The measuring half: one connection per phase, and the arithmetic that
//! turns what came back into a number worth printing.

use std::io::{self, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

use crate::proto::{self, Counted, Header, Mode};
use crate::socket;

/// Long enough for a busy peer, short enough that a route that is not there
/// is reported rather than waited on. The route was probed before this.
const CONNECT: Duration = Duration::from_secs(5);

/// Round trips taken before the timed ones, so the first sample is not the
/// one that also paid for the connection warming up.
const WARMUP_PINGS: usize = 5;

/// Samples below which the budget does not get a say: a handful of round
/// trips is a number, not a distribution.
const ENOUGH_PINGS: usize = 10;

/// Where the peer's half is listening, and how to reach it.
#[derive(Clone, Copy)]
pub struct Peer {
    pub address: SocketAddrV4,
    /// The address to bind on this side, which is what pins a measurement to
    /// one route. `None` when the peer was named directly and no route is
    /// being claimed.
    pub local: Option<Ipv4Addr>,
    pub token: [u8; 16],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// This machine to the peer.
    Up,
    /// The peer to this machine.
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

    /// Round trip time of a small payload on an established connection, which
    /// is the number a program on either machine actually waits for. Nagle is
    /// off, so a sample is one packet out and one back rather than the kernel
    /// deciding when to send.
    ///
    /// `samples` is a ceiling and `budget` is the other one: on a route where
    /// a round trip is milliseconds rather than fractions of one, a full set
    /// of samples would make the quickest phase the longest, so sampling
    /// stops early once there are enough of them to describe a distribution.
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

    /// Move zeros for `window` in one direction and report what landed.
    ///
    /// Whichever side receives is the side that counts, so this is the rate
    /// the bytes arrived at rather than the rate the sender handed them to
    /// its kernel — the two differ by however much the socket buffer was
    /// holding when time ran out.
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

    /// Tell the peer's half to exit. Best effort: it has an idle timeout for
    /// the times this does not arrive.
    pub fn bye(&self) {
        let _ = self.dial(Mode::Bye, Duration::ZERO);
    }
}

/// Parallel streams share one link, so their bytes add up but their times do
/// not: the rate is everything that arrived over the window the slowest
/// stream was open for. That is the conservative reading — a stream that
/// finished early is counted as if it had been idle for the rest.
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

/// Sorted round trips, which is the shape every statistic below wants.
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

    /// Nearest-rank: the value at least `percent` of the samples are at or
    /// below. With a hundred-odd samples the interpolating definitions are
    /// splitting hairs, and this one always returns a sample that happened.
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
