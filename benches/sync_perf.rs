//! TurboFox Suite E: Durability & Sync Barrier benchmark
//!
//! Evaluates physical storage fsync latency across dirty page volumes;
//! adheres strictly to Suite E geometry (6-byte keys, 0x24-byte values) and phase isolation

mod common;

use common::{BenchConfig, BenchDir, BenchSummary, new_histogram, print_json_summary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

const DEFAULT_SYNC_OPS_CAP: usize = 0x20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    Pages0x20,
    Pages0x0100,
    Pages0x0400,
}

impl Workload {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "0x0100" | "pages_0x0100" | "sync_0x0100" => Self::Pages0x0100,
            "0x0400" | "pages_0x0400" | "sync_0x0400" => Self::Pages0x0400,
            _ => Self::Pages0x20,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pages0x20 => "pages_0x20",
            Self::Pages0x0100 => "pages_0x0100",
            Self::Pages0x0400 => "pages_0x0400",
        }
    }

    pub fn target_pages(&self) -> u32 {
        match self {
            Self::Pages0x20 => 0x20,
            Self::Pages0x0100 => 0x0100,
            Self::Pages0x0400 => 0x0400,
        }
    }
}

pub fn make_suite_e_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

fn populate_to_target_pages(
    db: &TurboFox,
    target_pages: u32,
    val: &[u8; VAL_SIZE],
) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::new();
    let mut id = 0x00u32;

    while db.total_pages().unwrap_or(0x00) < target_pages {
        let key = make_suite_e_key(0x0001, id);
        let _ = db.set(&key, val).expect("pre-write failed");
        keys.push(key);
        id += 0x01;
    }

    // Ensure baseline is durably committed before starting timed measurement
    db.sync().expect("baseline sync barrier failed");
    keys
}

fn main() {
    let (config, workload_str) = BenchConfig::parse();
    let workload = match workload_str {
        Some(ref s) => Workload::from_str(s),
        None => Workload::Pages0x20,
    };

    let thread_count = config.threads.max(0x01);
    let sync_ops =
        if config.ops > DEFAULT_SYNC_OPS_CAP { DEFAULT_SYNC_OPS_CAP } else { config.ops.max(0x01) };
    let ops_per_thread = (sync_ops / thread_count).max(0x01);
    let effective_total_ops = ops_per_thread * thread_count;

    let (dir_guard, db_path) = BenchDir::new(config.db_path, "sync_perf");

    let cfg = TurboCfg::new(db_path);
    let db = TurboFox::create(cfg).expect("db initialization failed");

    let initial_val = [0x55u8; VAL_SIZE];
    let mutated_val = [0xAAu8; VAL_SIZE];

    // Pre-population phase: scale physical backing store to target page volume
    let keys = Arc::new(populate_to_target_pages(&db, workload.target_pages(), &initial_val));

    let key_count = keys.len();
    let chunk_size = (key_count / thread_count).max(0x01);

    let start_barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let dirty_barrier = Arc::new(Barrier::new(thread_count));
    let round_barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::with_capacity(thread_count);

    // Measurement phase: explicit sync barrier latency across dirty page volumes
    for t in 0x00..thread_count {
        let db_clone = db.clone();
        let keys_clone = keys.clone();
        let start_barrier_clone = start_barrier.clone();
        let dirty_barrier_clone = dirty_barrier.clone();
        let round_barrier_clone = round_barrier.clone();
        let start_idx = t * chunk_size;
        let end_idx = if t + 0x01 == thread_count {
            key_count
        } else {
            (start_idx + chunk_size).min(key_count)
        };

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();
            let thread_keys = &keys_clone[start_idx..end_idx];

            start_barrier_clone.wait();

            for iter in 0x00..ops_per_thread {
                let payload = if iter % 0x02 == 0x00 { mutated_val } else { initial_val };

                // Dirty the thread's memory-mapped page segment
                for key in thread_keys {
                    let _ = db_clone.set(key, &payload).expect("dirty write failed");
                }

                // Synchronize threads so full page volume is dirty before sync
                dirty_barrier_clone.wait();

                let t0 = Instant::now();
                db_clone.sync().expect("sync barrier failed");
                let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                let _ = hist.record(elapsed_nanos);

                // Synchronize threads before initiating next dirtying round
                round_barrier_clone.wait();
            }

            hist
        }));
    }

    start_barrier.wait();
    let bench_start = Instant::now();

    let mut combined_hist = new_histogram();
    for h in handles {
        let thread_hist = h.join().expect("worker thread panicked");
        combined_hist.add(&thread_hist).expect("failed to merge histograms");
    }

    let total_elapsed = bench_start.elapsed();
    let final_db_size = dir_guard.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "sync_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
