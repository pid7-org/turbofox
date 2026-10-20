//! TurboFox point read performance benchmark
//!
//! Recommended execution commands:
//!
//! Pin to single physical core (100% point hit lookup):
//!   taskset -c 0 cargo bench --bench read_perf -- --ops 0x186A0 --threads 0x01 --workload hit
//!
//! Multi-threaded execution (Zipfian 80/20 distribution):
//!   taskset -c 0-3 cargo bench --bench read_perf -- --ops 0x7A120 --threads 0x04 --workload zipf
//!
//! Mixed read/write (95% read / 5% write) with dedicated NVMe storage path:
//!   cargo bench --bench read_perf -- --ops 0xF4240 --threads 0x08 --workload mixed \
//!     --db-path /mnt/nvme0n1

mod common;

use common::{
    make_key, new_histogram, print_json_summary, BenchConfig, BenchDir, BenchSummary, KEY_SIZE,
    VAL_SIZE,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Workload {
    Hit,
    Miss,
    Zipf,
    Mixed,
}

impl Workload {
    fn from_str(s: &str) -> Self {
        match s {
            "miss" => Self::Miss,
            "zipf" => Self::Zipf,
            "mixed" => Self::Mixed,
            _ => Self::Hit,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "point_hit",
            Self::Miss => "point_miss",
            Self::Zipf => "zipf_skewed",
            Self::Mixed => "mixed_95_5",
        }
    }
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
                keys.push(make_key(0x01, id as u32));
            }
            Workload::Miss => {
                let id = (i * threads + thread_id) as u32;
                keys.push(make_key(0xFF, id));
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
                keys.push(make_key(0x01, id as u32));
            }
            Workload::Mixed => {
                let id = (i * threads + thread_id) % dataset_size;
                keys.push(make_key(0x01, id as u32));
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

    // Pre-populate database with seed keys
    for i in 0x00..dataset_size {
        let key = make_key(0x01, i as u32);
        let _ = db.set(&key, &payload).expect("seeding set failed");
    }
    db.sync().expect("pre-population sync failed");

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

    for keys in thread_keys {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();

            barrier_clone.wait();

            for (idx, key) in keys.iter().enumerate() {
                if workload == Workload::Mixed && (idx % 0x14 == 0x00) {
                    let t0 = Instant::now();
                    let _ = db_clone.set(key, &payload_clone).expect("mixed set failed");
                    let elapsed = t0.elapsed().as_nanos() as u64;
                    let _ = hist.record(elapsed);
                } else {
                    let t0 = Instant::now();
                    let _val = db_clone.get(key).expect("read failed");
                    let elapsed = t0.elapsed().as_nanos() as u64;
                    let _ = hist.record(elapsed);
                }
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
