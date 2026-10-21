//! TurboFox Suite A: Key-Value Ingestion & Mutation benchmark
//!
//! Evaluates raw append allocation versus existing entry overwrite performance;
//! adheres strictly to Suite A geometry (6-byte keys, 0x24-byte values)

mod common;

use common::{BenchConfig, BenchDir, BenchSummary, new_histogram, print_json_summary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    PureAppend,
    MixedMutation,
}

impl Workload {
    pub fn from_str(s: &str) -> Self {
        match s {
            "mixed" | "mixed_mutation" | "mutation" => Self::MixedMutation,
            _ => Self::PureAppend,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PureAppend => "pure_append",
            Self::MixedMutation => "mixed_mutation",
        }
    }
}

pub fn make_suite_a_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

enum WriteOp {
    Fresh([u8; KEY_SIZE]),
    Update([u8; KEY_SIZE]),
}

fn generate_pure_append_keys(thread_id: usize, count: usize) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    for i in 0x00..count {
        keys.push(make_suite_a_key(thread_id as u16, i as u32));
    }
    keys
}

fn generate_mixed_ops(thread_id: usize, count: usize, update_pool_size: usize) -> Vec<WriteOp> {
    let mut ops = Vec::with_capacity(count);
    let fresh_target = (count * 0x3C) / 0x64;
    let mut fresh_emitted = 0x00;
    let mut update_emitted = 0x00;

    for i in 0x00..count {
        let should_fresh = if fresh_emitted < fresh_target {
            let rem_total = count - i;
            let rem_fresh = fresh_target - fresh_emitted;
            (i * 0x3C) % 0x64 < 0x3C || rem_fresh >= rem_total
        } else {
            false
        };

        if should_fresh {
            let key = make_suite_a_key((thread_id as u16) | 0x8000, fresh_emitted as u32);
            ops.push(WriteOp::Fresh(key));
            fresh_emitted += 0x01;
        } else {
            let target_idx =
                if update_pool_size > 0x00 { update_emitted % update_pool_size } else { 0x00 };
            let key = make_suite_a_key(thread_id as u16, target_idx as u32);
            ops.push(WriteOp::Update(key));
            update_emitted += 0x01;
        }
    }
    ops
}

fn main() {
    let (cfg, workload_str) = BenchConfig::parse();
    let workload = Workload::from_str(workload_str.as_deref().unwrap_or("pure_append"));
    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "write_perf");
    let turbocfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(turbocfg).expect("failed to create turbofox instance");

    // Optional warmup pre-fill
    if cfg.warmup_ops > 0x00 {
        let warmup_val = vec![0xEEu8; VAL_SIZE];
        for i in 0x00..cfg.warmup_ops {
            let key = make_suite_a_key(0x7FFF, i as u32);
            let _ = db.set(&key, &warmup_val);
        }
        let _ = db.sync();
    }

    let fresh_val = vec![0xAAu8; VAL_SIZE];
    let update_val = vec![0xBBu8; VAL_SIZE];
    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    match workload {
        Workload::PureAppend => {
            let mut thread_keys = Vec::with_capacity(thread_count);
            for t in 0x00..thread_count {
                thread_keys.push(generate_pure_append_keys(t, ops_per_thread));
            }

            for keys in thread_keys {
                let db_clone = db.clone();
                let barrier_clone = barrier.clone();
                let payload = fresh_val.clone();

                handles.push(thread::spawn(move || {
                    let mut hist = new_histogram();
                    barrier_clone.wait();

                    for key in &keys {
                        let t0 = Instant::now();
                        let _ticket = db_clone.set(key, &payload).expect("append failed");
                        let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                        let _ = hist.record(elapsed_nanos);
                    }

                    hist
                }));
            }
        }
        Workload::MixedMutation => {
            let update_ops_per_thread = ops_per_thread - (ops_per_thread * 0x3C) / 0x64;
            let seed_val = vec![0x11u8; VAL_SIZE];

            // Pre-seed 0x28% existing keys prior to starting timed measurement phase
            for t in 0x00..thread_count {
                for i in 0x00..update_ops_per_thread {
                    let key = make_suite_a_key(t as u16, i as u32);
                    let _ = db.set(&key, &seed_val).expect("pre-seeding failed");
                }
            }
            db.sync().expect("pre-seeding sync failed");

            let mut thread_ops = Vec::with_capacity(thread_count);
            for t in 0x00..thread_count {
                thread_ops.push(generate_mixed_ops(t, ops_per_thread, update_ops_per_thread));
            }

            for ops in thread_ops {
                let db_clone = db.clone();
                let barrier_clone = barrier.clone();
                let fresh_payload = fresh_val.clone();
                let update_payload = update_val.clone();

                handles.push(thread::spawn(move || {
                    let mut hist = new_histogram();
                    barrier_clone.wait();

                    for op in &ops {
                        let t0 = Instant::now();
                        match op {
                            WriteOp::Fresh(k) => {
                                let _ = db_clone.set(k, &fresh_payload).expect("fresh failed");
                            }
                            WriteOp::Update(k) => {
                                let _ = db_clone.set(k, &update_payload).expect("update failed");
                            }
                        }
                        let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                        let _ = hist.record(elapsed_nanos);
                    }

                    hist
                }));
            }
        }
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
        benchmark: "write_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
