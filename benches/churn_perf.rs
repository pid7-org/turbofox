//! TurboFox Suite F: Sustained Churn & Slot Reclaim benchmark
//!
//! Evaluates slot vacancy reuse, Adjacent Array Block recycling, and storage stability;
//! adheres strictly to Suite F geometry (6-byte keys, 0x24-byte values) and phase isolation

mod common;

use common::{BenchConfig, BenchDir, BenchSummary, new_histogram, print_json_summary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

const BASELINE_PREFIX: u16 = 0x0001;
const CHURN_PREFIX: u16 = 0x0002;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    SteadyState,
}

impl Workload {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "steady_state" | "churn" | "steady_state_churn" | "sustained_churn" => {
                Self::SteadyState
            }
            _ => Self::SteadyState,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SteadyState => "steady_state_churn",
        }
    }
}

pub fn make_suite_f_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

fn main() {
    let (config, workload_str) = BenchConfig::parse();
    let workload = match workload_str {
        Some(ref s) => Workload::from_str(s),
        None => Workload::SteadyState,
    };

    let thread_count = config.threads.max(0x01);
    let total_ops = config.ops;
    // Each step performs 1 insert and 1 delete (2 operations total)
    let steps_per_thread = (total_ops / (thread_count * 0x02)).max(0x01);
    let effective_total_ops = steps_per_thread * thread_count * 0x02;

    let (dir_guard, db_path) = BenchDir::new(config.db_path, "churn_perf");

    let cfg = TurboCfg::new(db_path);
    let db = TurboFox::create(cfg).expect("db initialization failed");

    let payload = Arc::new([0x37u8; VAL_SIZE]);

    // Pre-population phase: ingest 0x64% baseline active working set
    for t in 0x00..thread_count {
        for i in 0x00..steps_per_thread {
            let id = (t * steps_per_thread + i) as u32;
            let key = make_suite_f_key(BASELINE_PREFIX, id);
            let _ = db.set(&key, &payload[..]).expect("baseline ingest failed");
        }
    }

    // Issue explicit sync barrier prior to starting timed churn runs
    db.sync().expect("pre-measurement sync barrier failed");

    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    // Measurement phase: 0x32% inserts alternating with 0x32% deletions
    for t in 0x00..thread_count {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();

            barrier_clone.wait();

            for i in 0x00..steps_per_thread {
                let id = (t * steps_per_thread + i) as u32;
                let insert_key = make_suite_f_key(CHURN_PREFIX, id);
                let delete_key = make_suite_f_key(BASELINE_PREFIX, id);

                // Insert operation: evaluates slot vacancy reuse and tombstone replacement
                let t_insert = Instant::now();
                let _ = db_clone.set(&insert_key, &payload_clone[..]).expect("churn set failed");
                let elapsed_insert = t_insert.elapsed().as_nanos() as u64;
                let _ = hist.record(elapsed_insert);

                // Delete operation: invalidates active slot and generates tombstones
                let t_del = Instant::now();
                let _ = db_clone.del(&delete_key).expect("churn del failed");
                let elapsed_del = t_del.elapsed().as_nanos() as u64;
                let _ = hist.record(elapsed_del);
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

    // Post-run storage sync barrier excluded from timing records
    db.sync().expect("post-run storage sync failed");
    let final_db_size = dir_guard.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "churn_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
