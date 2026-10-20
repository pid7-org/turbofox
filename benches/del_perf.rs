//! TurboFox delete performance benchmark
//!
//! Recommended execution commands:
//!
//! Pin to single physical core (100% disjoint delete hit):
//!   taskset -c 0 cargo bench --bench del_perf -- --ops 0x186A0 --threads 0x01 --workload hit
//!
//! Multi-threaded execution under high key contention (shared pool):
//!   taskset -c 0-3 cargo bench --bench del_perf -- --ops 0x7A120 --threads 0x04 \
//!     --workload contention
//!
//! Steady-state churn (50% insert / 50% delete) with dedicated NVMe storage path:
//!   cargo bench --bench del_perf -- --ops 0xF4240 --threads 0x08 --workload churn \
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

const CONTENTION_POOL: usize = 0x40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Workload {
    Hit,
    Miss,
    Churn,
    Contention,
}

impl Workload {
    fn from_str(s: &str) -> Self {
        match s {
            "miss" => Self::Miss,
            "churn" => Self::Churn,
            "contention" => Self::Contention,
            _ => Self::Hit,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "delete_hit",
            Self::Miss => "delete_miss",
            Self::Churn => "churn_insert_delete",
            Self::Contention => "high_contention",
        }
    }
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
                keys.push(make_key(0x01, id as u32));
            }
            Workload::Miss => {
                let id = thread_id * count + i;
                keys.push(make_key(0xFF, id as u32));
            }
            Workload::Churn => {
                let id = thread_id * count + i;
                keys.push(make_key(0x01, id as u32));
            }
            Workload::Contention => {
                let id = (i * threads + thread_id) % CONTENTION_POOL;
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
    let dataset_size = if workload == Workload::Contention {
        CONTENTION_POOL
    } else {
        effective_total_ops
    };

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "del_perf");
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
        thread_keys.push(pregenerate_del_keys(workload, t, ops_per_thread, thread_count));
    }

    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

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
                    let churn_key = make_key(0x02, new_id);
                    let _ = db_clone.set(&churn_key, &payload_clone).expect("churn set failed");
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("churn del failed");
                    let elapsed = t0.elapsed().as_nanos() as u64;
                    let _ = hist.record(elapsed);
                } else {
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("del failed");
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
