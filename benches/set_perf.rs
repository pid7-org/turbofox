//! TurboFox Key-Value SET / Ingestion Benchmark
//!
//! Evaluates sustained write throughput and tail latency across 1T and 4T concurrency

mod common;

use common::{
    BenchConfig, BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, VAL_SIZE, make_key, make_val,
    new_histogram, print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn main() {
    let cfg = BenchConfig::parse();
    let (bench_dir, db_path) = BenchDir::new(cfg.db_path.clone(), "set_perf");

    let turbo_cfg = TurboCfg::manual(&db_path);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let total_ops = ops_per_thread * thread_count;

    // Warmup phase
    if cfg.warmup_ops > 0x00 {
        let warmup_per_thread = cfg.warmup_ops / thread_count;
        let mut handles = Vec::with_capacity(thread_count);
        for tid in 0x00..thread_count {
            let db_clone = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                let val = make_val(0xDEAD);
                for i in 0x00..warmup_per_thread {
                    let key = make_key(0xEEEE, tid as u16, i as u32);
                    let _ = db_clone.set(&key, &val);
                }
            }));
        }
        for h in handles {
            h.join().expect("warmup thread panicked");
        }
    }

    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::with_capacity(thread_count);

    let start_instant = Arc::new(std::sync::atomic::AtomicU64::new(0x00));
    let start_all = Instant::now();

    for tid in 0x00..thread_count {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier);
        let start_time_ref = Arc::clone(&start_instant);

        // Pre-generate keys and payload outside timed region
        let mut keys: Vec<[u8; KEY_SIZE]> = Vec::with_capacity(ops_per_thread);
        for i in 0x00..ops_per_thread {
            keys.push(make_key(HIT_PREFIX, tid as u16, i as u32));
        }
        let val: [u8; VAL_SIZE] = make_val(tid as u32);

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();
            bar.wait();
            if tid == 0x00 {
                start_time_ref.store(
                    start_all.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::SeqCst,
                );
            }

            for key in &keys {
                let t0 = Instant::now();
                let ticket = db_clone.set(key, &val).expect("set failed");
                std::hint::black_box(&ticket);
                let elapsed_ns = t0.elapsed().as_nanos() as u64;
                hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            }

            hist
        }));
    }

    let mut merged_hist = new_histogram();
    for h in handles {
        let thread_hist = h.join().expect("bench thread panicked");
        merged_hist.add(&thread_hist).expect("failed to merge histograms");
    }

    let total_elapsed = start_all.elapsed();
    let db_size = bench_dir.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "set_perf",
        workload: "insert",
        total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &merged_hist,
        db_size_bytes: db_size,
    };

    print_json_summary(&summary);
}
