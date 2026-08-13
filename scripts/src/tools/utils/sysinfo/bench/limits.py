MIN_RUNS = 3
MAX_RUNS = 6
RSD_THRESHOLD = 2.5

LOAD_LIMIT = 0.30
MIN_FREE_DISK = 0.20
THROTTLE_MARGIN = 5.0

COOLDOWN_SECONDS = 240
COOLDOWN_INTERVAL = 15

NOISE_FLOOR_PCT = 2.0
NOISE_MAD_FACTOR = 3.0

# Metrics run once carry no spread of their own, so their band would otherwise
# collapse onto the 2% floor -- and fio and glmark2, the tools that are run once,
# are exactly the ones that do not repeat to 2%. Widening the floor for a single
# sample stops the least replicated metrics producing the most false verdicts.
NOISE_SINGLE_PCT = 8.0

# Memory-backed filesystems. Benchmarking one measures RAM and files it as disk.
MEMORY_FILESYSTEMS = ("tmpfs", "ramfs", "devtmpfs", "hugetlbfs")

REGRESSION_PCT = 10.0

FIO_RAMP_SECONDS = 5
FIO_RUNTIME_SECONDS = 20
FIO_IODEPTH = 64

SUSTAINED_SECONDS = {"quick": 0, "standard": 60, "heavy": 120}
FIO_SIZE = {"quick": "", "standard": "1g", "heavy": "8g"}

# Bytes each write stage is allowed to put through the drive. Read stages stay
# time based, because reads cost no endurance; write stages are size bounded so
# the volume is known before the run rather than discovered afterwards. Without
# this fio's time_based loops for its whole runtime and writes whatever the
# drive can absorb, which on a fast NVMe is an order of magnitude past the cap.
FIO_WRITE_SIZE = {
    "quick": {},
    "standard": {"seq-write": "6g", "rand-write": "2g"},
    "heavy": {"seq-write": "20g", "rand-write": "6g"},
}

WRITE_BUDGET = {"quick": 0, "standard": 30 * 1024**3, "heavy": 70 * 1024**3}
