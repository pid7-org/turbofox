//! TurboFox Key-Value GET Benchmark
//!
//! Evaluates read throughput and tail latency across 1T and 4T under a 60% hit / 40% miss workload

mod common;

use common::{
    BenchConfig, BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, generate_60_40_probe_keys,
    make_key, make_val, new_histogram, print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn main() {
    let cfg = BenchConfig::parse();
    let (bench_dir, db_path) = BenchDir::new(cfg.db_path.clone(), "get_perf");

    let turbo_cfg = TurboCfg::manual(&db_path);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let total_ops = ops_per_thread * thread_count;
    let hit_pool_per_thread = ops_per_thread;

    // Pre-populate hit dataset
    for tid in 0x00..thread_count {
        let val = make_val(tid as u32);
        for i in 0x00..hit_pool_per_thread {
            let key = make_key(HIT_PREFIX, tid as u16, i as u32);
            db.set(&key, &val).expect("prepopulate set failed");
        }
    }
    db.sync().expect("prepopulate sync failed");

    // Pre-generate probe keys (60% hit, 40% miss) per thread outside timed region
    let mut thread_probes: Vec<Vec<[u8; KEY_SIZE]>> = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        thread_probes.push(generate_60_40_probe_keys(
            tid,
            ops_per_thread,
            hit_pool_per_thread,
        ));
    }

    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::with_capacity(thread_count);
    let start_all = Instant::now();

    for probes in thread_probes {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();
            bar.wait();

            for key in &probes {
                let t0 = Instant::now();
                let res = db_clone.get(key).expect("get failed");
                std::hint::black_box(&res);
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
        benchmark: "get_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &merged_hist,
        db_size_bytes: db_size,
    };

    print_json_summary(&summary);
}
