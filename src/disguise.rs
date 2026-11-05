use std::time::{SystemTime, UNIX_EPOCH};

use crate::render::{Entry, Level};

pub struct Rng(u64);

impl Rng {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9);
        Rng(mix(seed) | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn pick(modules: &[String], h: u64) -> String {
    if modules.is_empty() {
        "main".into()
    } else {
        modules[(h % modules.len() as u64) as usize].clone()
    }
}

/// A stable level + module for a book line, so it keeps the same disguise when redrawn.
pub fn tag(para: usize, start: usize, modules: &[String]) -> (Level, String) {
    let h = mix(((para as u64) << 24) ^ start as u64);
    let level = if h.is_multiple_of(11) { Level::Debug } else { Level::Info };
    (level, pick(modules, h >> 8))
}

/// A plausible line of service noise for boss mode.
pub fn fake_entry(rng: &mut Rng, modules: &[String], time: i64) -> Entry {
    let (level, text) = match rng.range(0, 13) {
        0 => (Level::Info, format!("GET /api/v1/health 200 {}ms", rng.range(1, 9))),
        1 => (Level::Info, format!("POST /api/v1/jobs 201 {}ms", rng.range(12, 180))),
        2 => (Level::Info, format!("GET /api/v1/tasks?page={} 200 {}ms", rng.range(1, 40), rng.range(8, 95))),
        3 => (Level::Debug, format!("cache hit ratio=0.{:02} keys={}", rng.range(80, 99), rng.range(1000, 90000))),
        4 => (Level::Info, format!("flushed {} records to sink in {}ms", rng.range(20, 4000), rng.range(3, 400))),
        5 => (Level::Debug, format!("heartbeat ok peer=10.0.{}.{}:7000 rtt={}ms", rng.range(0, 9), rng.range(2, 250), rng.range(1, 30))),
        6 => (Level::Info, format!("compaction finished: {} segments merged", rng.range(2, 24))),
        7 => (Level::Debug, format!("gc pause {}ms heap={}MB", rng.range(1, 40), rng.range(180, 900))),
        8 => (Level::Info, format!("worker-{} picked task #{}", rng.range(1, 8), rng.range(10000, 99999))),
        9 => (Level::Info, format!("task #{} done in {}.{}s", rng.range(10000, 99999), rng.range(0, 12), rng.range(0, 9))),
        10 => (Level::Warn, format!("retrying upstream connection (attempt {}/5)", rng.range(1, 3))),
        11 => (Level::Info, format!("checkpoint saved offset={}", rng.range(100000, 9999999))),
        12 => (Level::Debug, format!("pool stats active={} idle={} waiting=0", rng.range(1, 16), rng.range(0, 16))),
        _ => (Level::Info, format!("synced {} files ({} KiB)", rng.range(1, 300), rng.range(4, 9000))),
    };
    let module = pick(modules, rng.next());
    Entry { time, level, module, text }
}
