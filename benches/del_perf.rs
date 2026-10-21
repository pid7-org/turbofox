//! TurboFox Suite C: Deletion & Tombstone benchmark
//!
//! Evaluates OCC slot spinlocks, deletion tombstone markers, and bucket reorganization;
//! adheres strictly to Suite C geometry (6-byte keys, 0x24-byte values) and phase isolation

mod common;

use common::{BenchConfig, BenchDir, BenchSummary, new_histogram, print_json_summary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

const HIT_PREFIX: u16 = 0x0001;
const MISS_PREFIX: u16 = 0xFFFF;
const CONTENTION_POOL: usize = 0x40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    Hit,
    Miss,
    Mixed,
    Contention,
    Churn,
}

impl Workload {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "miss" | "delete_miss" => Self::Miss,
            "mixed" | "mixed_delete" | "mixed_hit_miss" => Self::Mixed,
            "contention" | "high_contention" => Self::Contention,
            "churn" | "churn_insert_delete" => Self::Churn,
            _ => Self::Hit,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "delete_hit",
            Self::Miss => "delete_miss",
            Self::Mixed => "mixed_delete",
            Self::Contention => "high_contention",
            Self::Churn => "churn_insert_delete",
        }
    }
}

pub fn make_suite_c_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

fn pregenerate_del_keys(
    workload: Workload,
    thread_id: usize,
    count: usize,
    threads: usize,
) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    for i in 0x00..count {
        match workload {
            Workload::Hit => {
                let id = thread_id * count + i;
                keys.push(make_suite_c_key(HIT_PREFIX, id as u32));
            }
            Workload::Miss => {
                let id = thread_id * count + i;
                keys.push(make_suite_c_key(MISS_PREFIX, id as u32));
            }
            Workload::Mixed => {
                // 0x50% (80%) hits interleaved with 0x14% (20%) misses
                if i % 0x05 < 0x04 {
                    let id = thread_id * count + i;
                    keys.push(make_suite_c_key(HIT_PREFIX, id as u32));
                } else {
                    let id = thread_id * count + i;
                    keys.push(make_suite_c_key(MISS_PREFIX, id as u32));
                }
            }
            Workload::Contention => {
                let id = (i * threads + thread_id) % CONTENTION_POOL;
                keys.push(make_suite_c_key(HIT_PREFIX, id as u32));
            }
            Workload::Churn => {
                let id = thread_id * count + i;
                keys.push(make_suite_c_key(HIT_PREFIX, id as u32));
            }
        }
    }
    keys
}

fn main() {
    let (cfg, workload_arg) = BenchConfig::parse();
    let workload = workload_arg.as_deref().map(Workload::from_str).unwrap_or(Workload::Hit);

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;
    let dataset_size =
        if workload == Workload::Contention { CONTENTION_POOL } else { effective_total_ops };

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "del_perf");
    let turbocfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(turbocfg).expect("failed to create turbofox instance");

    let payload = vec![0x5Au8; VAL_SIZE];

    // Pre-population phase: Ingest 0x64% dataset and issue explicit sync barrier
    for i in 0x00..dataset_size {
        let key = make_suite_c_key(HIT_PREFIX, i as u32);
        let _ = db.set(&key, &payload).expect("pre-population write failed");
    }
    db.sync().expect("pre-population sync barrier failed");

    let mut thread_keys = Vec::with_capacity(thread_count);
    for t in 0x00..thread_count {
        thread_keys.push(pregenerate_del_keys(workload, t, ops_per_thread, thread_count));
    }

    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    // Measurement phase: exclusive deletion timing isolated by barrier
    for (t, keys) in thread_keys.into_iter().enumerate() {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();

            barrier_clone.wait();

            for (idx, key) in keys.iter().enumerate() {
                if workload == Workload::Churn {
                    let new_id = (t * ops_per_thread + idx) as u32;
                    let churn_key = make_suite_c_key(0x0002, new_id);
                    let _ = db_clone.set(&churn_key, &payload_clone).expect("churn set failed");
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("churn del failed");
                    let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                    let _ = hist.record(elapsed_nanos);
                } else {
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("point del failed");
                    let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                    let _ = hist.record(elapsed_nanos);
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

    // Post-run procedure: flush updated slot headers and metadata
    db.sync().expect("post-run sync barrier failed");
    let final_db_size = dir_guard.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "del_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
