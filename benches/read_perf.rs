//! TurboFox Suite B: Point Retrieval benchmark
//!
//! Evaluates index lookup overhead, SIMD bucket probe efficiency, and CPU cache residency;
//! adheres strictly to Suite B geometry (6-byte keys, 0x24-byte values) and read-only isolation

mod common;

use common::{new_histogram, print_json_summary, BenchConfig, BenchDir, BenchSummary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

const HIT_PREFIX: u16 = 0x0001;
const MISS_PREFIX: u16 = 0xFFFF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    Hit,
    Miss,
    Mixed,
    Zipf,
}

impl Workload {
    pub fn from_str(s: &str) -> Self {
        match s {
            "miss" | "point_miss" => Self::Miss,
            "mixed" | "mixed_hit_miss" => Self::Mixed,
            "zipf" | "zipf_skewed" => Self::Zipf,
            _ => Self::Hit,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "point_hit",
            Self::Miss => "point_miss",
            Self::Mixed => "mixed_hit_miss",
            Self::Zipf => "zipf_skewed",
        }
    }
}

pub fn make_suite_b_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

fn pregenerate_probe_keys(
    workload: Workload,
    thread_id: usize,
    count: usize,
    dataset_size: usize,
    threads: usize,
) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    let mut seed =
        0x1234_5678_9ABC_DEF0u64 ^ ((thread_id as u64 + 0x01) * 0x5851_F42D_4C95_7F2Du64);

    for i in 0x00..count {
        match workload {
            Workload::Hit => {
                let id = (i * threads + thread_id) % dataset_size;
                keys.push(make_suite_b_key(HIT_PREFIX, id as u32));
            }
            Workload::Miss => {
                let id = (i * threads + thread_id) as u32;
                keys.push(make_suite_b_key(MISS_PREFIX, id));
            }
            Workload::Mixed => {
                // 0x50% (80%) hits interleaved with 0x14% (20%) misses
                if i % 0x05 < 0x04 {
                    let id = (i * threads + thread_id) % dataset_size;
                    keys.push(make_suite_b_key(HIT_PREFIX, id as u32));
                } else {
                    let id = (i * threads + thread_id) as u32;
                    keys.push(make_suite_b_key(MISS_PREFIX, id));
                }
            }
            Workload::Zipf => {
                seed ^= seed << 0x0D;
                seed ^= seed >> 0x07;
                seed ^= seed << 0x11;
                let pct = (seed % 0x64) as usize;
                let id = if pct < 0x50 {
                    let top_pool = ((dataset_size * 0x14) / 0x64).max(0x01);
                    (seed as usize) % top_pool
                } else {
                    (seed as usize) % dataset_size
                };
                keys.push(make_suite_b_key(HIT_PREFIX, id as u32));
            }
        }
    }
    keys
}

fn main() {
    let (cfg, workload_arg) = BenchConfig::parse();
    let workload = workload_arg
        .as_deref()
        .map(Workload::from_str)
        .unwrap_or(Workload::Hit);

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;
    let dataset_size = effective_total_ops;

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "read_perf");
    let turbocfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(turbocfg).expect("failed to create turbofox instance");

    let payload = vec![0x5Au8; VAL_SIZE];

    // Pre-population phase: Ingest 0x64% dataset and issue explicit sync barrier
    for i in 0x00..dataset_size {
        let key = make_suite_b_key(HIT_PREFIX, i as u32);
        let _ = db.set(&key, &payload).expect("pre-population write failed");
    }
    db.sync().expect("pre-population sync barrier failed");

    let mut thread_keys = Vec::with_capacity(thread_count);
    for t in 0x00..thread_count {
        thread_keys.push(pregenerate_probe_keys(
            workload,
            t,
            ops_per_thread,
            dataset_size,
            thread_count,
        ));
    }

    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    // Measurement phase: Strictly read-only execution; zero writes or flushes during timer window
    for keys in thread_keys {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();
            barrier_clone.wait();

            for key in &keys {
                let t0 = Instant::now();
                let _val = db_clone.get(key).expect("point read failed");
                let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                let _ = hist.record(elapsed_nanos);
            }

            hist
        }));
    }

    barrier.wait();
    let bench_start = Instant::now();

    let mut combined_hist = new_histogram();
    for h in handles {
        let thread_hist = h.join().expect("worker thread panicked");
        combined_hist.add(&thread_hist).expect("failed to merge histograms");
    }

    let total_elapsed = bench_start.elapsed();
    let final_db_size = dir_guard.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "read_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
