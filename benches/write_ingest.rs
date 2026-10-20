//! TurboFox write ingestion benchmark
//!
//! Recommended execution commands:
//!
//! Pin to single physical core (eliminates scheduler migration):
//!   taskset -c 0 cargo bench --bench write_ingest -- --ops 0x186A0 --threads 0x01
//!
//! Multi-threaded execution on isolated cores:
//!   taskset -c 0-3 cargo bench --bench write_ingest -- --ops 0x7A120 --threads 0x04
//!
//! Dedicated NVMe mount on AWS EC2 (e.g. /mnt/nvme0n1):
//!   cargo bench --bench write_ingest -- --ops 0xF4240 --threads 0x08 --db-path /mnt/nvme0n1

mod common;

use common::{
    new_histogram, print_json_summary, BenchConfig, BenchDir, BenchSummary, KEY_SIZE, VAL_SIZE,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn pregenerate_keys(thread_id: usize, count: usize) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    for i in 0x00..count {
        keys.push(common::make_key(thread_id as u32, i as u32));
    }
    keys
}

fn main() {
    let (cfg, _) = BenchConfig::parse();
    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "write_ingest");
    let turbocfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(turbocfg).expect("failed to create turbofox instance");

    // Warmup cycle if specified
    if cfg.warmup_ops > 0x00 {
        let warmup_payload = vec![0xEEu8; VAL_SIZE];
        for i in 0x00..cfg.warmup_ops {
            let key = common::make_key(0xFFFF_FFFF, i as u32);
            let _ = db.set(&key, &warmup_payload);
        }
        let _ = db.sync();
    }

    let mut thread_keys = Vec::with_capacity(thread_count);
    for t in 0x00..thread_count {
        thread_keys.push(pregenerate_keys(t, ops_per_thread));
    }

    let payload = vec![0xAAu8; VAL_SIZE];
    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    for keys in thread_keys {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();

            barrier_clone.wait();

            for key in &keys {
                let t0 = Instant::now();
                let _ticket = db_clone.set(key, &payload_clone).expect("write failed");
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
        benchmark: "write_ingest",
        workload: "sequential_append",
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
