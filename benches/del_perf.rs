//! TurboFox Key-Value DEL Benchmark
//!
//! Evaluates delete throughput and tail latency across 1T and 4T under a 60% hit / 40% miss workload

mod common;

use common::{
    BenchConfig, BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, MISS_PREFIX, make_key, make_val,
    new_histogram, print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn generate_del_60_40_keys(thread_id: usize, count: usize) -> (Vec<[u8; KEY_SIZE]>, usize) {
    let mut keys = Vec::with_capacity(count);
    let hit_target = (count * 0x3C) / 0x64;
    let mut hit_idx = 0x00;
    let mut miss_idx = 0x00;

    for i in 0x00..count {
        let is_hit = if hit_idx < hit_target {
            let remaining_total = count - i;
            let remaining_hits = hit_target - hit_idx;
            (i * 0x3C) % 0x64 < 0x3C || remaining_hits >= remaining_total
        } else {
            false
        };

        if is_hit {
            keys.push(make_key(HIT_PREFIX, thread_id as u16, hit_idx as u32));
            hit_idx += 0x01;
        } else {
            keys.push(make_key(MISS_PREFIX, thread_id as u16, miss_idx as u32));
            miss_idx += 0x01;
        }
    }

    (keys, hit_target)
}

fn main() {
    let cfg = BenchConfig::parse();
    let (bench_dir, db_path) = BenchDir::new(cfg.db_path.clone(), "del_perf");

    let turbo_cfg = TurboCfg::manual(&db_path);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let total_ops = ops_per_thread * thread_count;

    // Generate keys and identify required prepopulation count
    let mut thread_keys = Vec::with_capacity(thread_count);
    let mut hit_counts = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        let (keys, hits) = generate_del_60_40_keys(tid, ops_per_thread);
        thread_keys.push(keys);
        hit_counts.push(hits);
    }

    // Pre-populate required hit keys
    for (tid, &hits) in hit_counts.iter().enumerate().take(thread_count) {
        let val = make_val(tid as u32);
        for i in 0x00..hits {
            let key = make_key(HIT_PREFIX, tid as u16, i as u32);
            db.set(&key, &val).expect("prepopulate set failed");
        }
    }
    db.sync().expect("prepopulate sync failed");

    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::with_capacity(thread_count);
    let start_all = Instant::now();

    for keys in thread_keys {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            let mut hist = new_histogram();
            bar.wait();

            for key in &keys {
                let t0 = Instant::now();
                let res = db_clone.del(key).expect("del failed");
                std::hint::black_box(res);
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
        benchmark: "del_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &merged_hist,
        db_size_bytes: db_size,
    };

    print_json_summary(&summary);
}
