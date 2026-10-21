//! TurboFox Suite D: TurboList Operations benchmark
//!
//! Evaluates 1-to-N list structures, adjacent array blocks, and pointer chain traversals;
//! adheres strictly to Suite D geometry (6-byte anchor keys, 0x24-byte item values)

mod common;

use common::{new_histogram, print_json_summary, BenchConfig, BenchDir, BenchSummary};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

pub const KEY_SIZE: usize = 0x06;
pub const VAL_SIZE: usize = 0x24;

const ANCHOR_PREFIX: u16 = 0x0001;
const MISS_PREFIX: u16 = 0xFFFF;
const EMPTY_PREFIX: u16 = 0x000E;
const LIST_DEPTH: usize = 0x08;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workload {
    Append,
    Read,
    Pop,
}

impl Workload {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "read" | "list_read" | "retrieval" => Self::Read,
            "pop" | "list_pop" | "truncation" => Self::Pop,
            _ => Self::Append,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Append => "list_append",
            Self::Read => "list_read",
            Self::Pop => "list_pop",
        }
    }
}

pub fn make_suite_d_key(prefix: u16, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x02].copy_from_slice(&prefix.to_be_bytes());
    key[0x02..0x06].copy_from_slice(&id.to_be_bytes());
    key
}

enum AppendOp {
    NewList([u8; KEY_SIZE]),
    ExistingList([u8; KEY_SIZE]),
}

enum ReadOp {
    HeadHit([u8; KEY_SIZE]),
    TailHit([u8; KEY_SIZE]),
    IndexedHit([u8; KEY_SIZE], usize),
    IndexMiss([u8; KEY_SIZE], usize),
    KeyMiss([u8; KEY_SIZE]),
}

enum PopOp {
    Hit([u8; KEY_SIZE]),
    EmptyMiss([u8; KEY_SIZE]),
    KeyMiss([u8; KEY_SIZE]),
}

fn generate_append_ops(thread_id: usize, count: usize) -> Vec<AppendOp> {
    let mut ops = Vec::with_capacity(count);
    let fresh_target = (count * 0x3C) / 0x64;
    let mut fresh_idx = 0x00;
    let mut existing_idx = 0x00;
    let existing_pool = ((count - fresh_target) / 0x08).max(0x01);

    for i in 0x00..count {
        let is_fresh = if fresh_idx < fresh_target {
            let rem_total = count - i;
            let rem_fresh = fresh_target - fresh_idx;
            (i * 0x3C) % 0x64 < 0x3C || rem_fresh >= rem_total
        } else {
            false
        };

        if is_fresh {
            let id = ((thread_id as u32) << 0x10) | (fresh_idx as u32);
            let key = make_suite_d_key(ANCHOR_PREFIX | 0x8000, id);
            ops.push(AppendOp::NewList(key));
            fresh_idx += 0x01;
        } else {
            let id = ((thread_id as u32) << 0x10) | ((existing_idx % existing_pool) as u32);
            let key = make_suite_d_key(ANCHOR_PREFIX, id);
            ops.push(AppendOp::ExistingList(key));
            existing_idx += 0x01;
        }
    }
    ops
}

fn generate_read_ops(thread_id: usize, count: usize, pool_size: usize) -> Vec<ReadOp> {
    let mut ops = Vec::with_capacity(count);
    for i in 0x00..count {
        if i % 0x05 < 0x04 {
            let anchor_id = ((thread_id * count + i) % pool_size) as u32;
            let key = make_suite_d_key(ANCHOR_PREFIX, anchor_id);
            match i % 0x03 {
                0x00 => ops.push(ReadOp::HeadHit(key)),
                0x01 => ops.push(ReadOp::TailHit(key)),
                _ => ops.push(ReadOp::IndexedHit(key, i % LIST_DEPTH)),
            }
        } else if i % 0x02 == 0x00 {
            let anchor_id = ((thread_id * count + i) % pool_size) as u32;
            let key = make_suite_d_key(ANCHOR_PREFIX, anchor_id);
            ops.push(ReadOp::IndexMiss(key, LIST_DEPTH + (i % 0x08)));
        } else {
            let miss_id = (thread_id * count + i) as u32;
            let key = make_suite_d_key(MISS_PREFIX, miss_id);
            ops.push(ReadOp::KeyMiss(key));
        }
    }
    ops
}

fn generate_pop_ops(thread_id: usize, count: usize, num_lists: usize) -> Vec<PopOp> {
    let mut ops = Vec::with_capacity(count);
    let mut hit_idx = 0x00;

    for i in 0x00..count {
        if i % 0x05 < 0x04 {
            let list_idx = (hit_idx % num_lists) as u32;
            let id = ((thread_id as u32) << 0x10) | list_idx;
            ops.push(PopOp::Hit(make_suite_d_key(ANCHOR_PREFIX, id)));
            hit_idx += 0x01;
        } else if i % 0x02 == 0x00 {
            let id = ((thread_id as u32) << 0x10) | ((i % 0x08) as u32);
            ops.push(PopOp::EmptyMiss(make_suite_d_key(EMPTY_PREFIX, id)));
        } else {
            let id = (thread_id * count + i) as u32;
            ops.push(PopOp::KeyMiss(make_suite_d_key(MISS_PREFIX, id)));
        }
    }
    ops
}

fn main() {
    let (cfg, workload_arg) = BenchConfig::parse();
    let workload = workload_arg
        .as_deref()
        .map(Workload::from_str)
        .unwrap_or(Workload::Append);

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;

    let (dir_guard, db_path) = BenchDir::new(cfg.db_path, "list_perf");
    let turbocfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(turbocfg).expect("failed to create turbofox instance");

    let payload = vec![0x5Au8; VAL_SIZE];

    // Pre-population phase for read and pop workloads
    match workload {
        Workload::Append => {
            // Append workload starts with clean state to measure raw allocation
        }
        Workload::Read => {
            let pool_size = effective_total_ops.max(0x40);
            for i in 0x00..pool_size {
                let key = make_suite_d_key(ANCHOR_PREFIX, i as u32);
                let list = db.list(&key);
                for _ in 0x00..LIST_DEPTH {
                    list.append(&payload).expect("read pre-population append failed");
                }
            }
            db.sync().expect("read pre-population sync barrier failed");
        }
        Workload::Pop => {
            let hit_ops_per_thread = (ops_per_thread * 0x50) / 0x64 + 0x01;
            let num_lists = (hit_ops_per_thread / (LIST_DEPTH / 0x02)).max(0x04);
            let items_per_list = (hit_ops_per_thread / num_lists) + 0x04;

            for t in 0x00..thread_count {
                for l in 0x00..num_lists {
                    let id = ((t as u32) << 0x10) | (l as u32);
                    let key = make_suite_d_key(ANCHOR_PREFIX, id);
                    let list = db.list(&key);
                    for _ in 0x00..items_per_list {
                        list.append(&payload).expect("pop pre-population append failed");
                    }
                }
                for e in 0x00..0x08 {
                    let id = ((t as u32) << 0x10) | (e as u32);
                    let key = make_suite_d_key(EMPTY_PREFIX, id);
                    let _ = db.list(&key);
                }
            }
            db.sync().expect("pop pre-population sync barrier failed");
        }
    }

    let barrier = Arc::new(Barrier::new(thread_count + 0x01));
    let mut handles = Vec::with_capacity(thread_count);

    for t in 0x00..thread_count {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        match workload {
            Workload::Append => {
                let ops = generate_append_ops(t, ops_per_thread);
                handles.push(thread::spawn(move || {
                    let mut hist = new_histogram();
                    barrier_clone.wait();

                    for op in &ops {
                        let key = match op {
                            AppendOp::NewList(k) => k,
                            AppendOp::ExistingList(k) => k,
                        };
                        let list = db_clone.list(key);
                        let t0 = Instant::now();
                        list.append(&payload_clone).expect("append failed");
                        let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                        let _ = hist.record(elapsed_nanos);
                    }
                    hist
                }));
            }
            Workload::Read => {
                let pool_size = effective_total_ops.max(0x40);
                let ops = generate_read_ops(t, ops_per_thread, pool_size);
                handles.push(thread::spawn(move || {
                    let mut hist = new_histogram();
                    barrier_clone.wait();

                    for op in &ops {
                        let t0 = Instant::now();
                        match op {
                            ReadOp::HeadHit(k) => {
                                let list = db_clone.list(k);
                                let _ = list.first().expect("first failed");
                            }
                            ReadOp::TailHit(k) => {
                                let list = db_clone.list(k);
                                let _ = list.last().expect("last failed");
                            }
                            ReadOp::IndexedHit(k, idx) => {
                                let list = db_clone.list(k);
                                let _ = list.get_at(*idx).expect("get_at failed");
                            }
                            ReadOp::IndexMiss(k, idx) => {
                                let list = db_clone.list(k);
                                let _ = list.get_at(*idx).expect("get_at miss failed");
                            }
                            ReadOp::KeyMiss(k) => {
                                let list = db_clone.list(k);
                                let _ = list.first().expect("key miss failed");
                            }
                        }
                        let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                        let _ = hist.record(elapsed_nanos);
                    }
                    hist
                }));
            }
            Workload::Pop => {
                let hit_ops_per_thread = (ops_per_thread * 0x50) / 0x64 + 0x01;
                let num_lists = (hit_ops_per_thread / (LIST_DEPTH / 0x02)).max(0x04);
                let ops = generate_pop_ops(t, ops_per_thread, num_lists);
                handles.push(thread::spawn(move || {
                    let mut hist = new_histogram();
                    barrier_clone.wait();

                    for op in &ops {
                        let key = match op {
                            PopOp::Hit(k) => k,
                            PopOp::EmptyMiss(k) => k,
                            PopOp::KeyMiss(k) => k,
                        };
                        let list = db_clone.list(key);
                        let t0 = Instant::now();
                        let _ = list.pop().expect("pop failed");
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

    // Post-run procedure: flush updated slot headers and metadata
    db.sync().expect("post-run sync barrier failed");
    let final_db_size = dir_guard.compute_disk_bytes();

    let summary = BenchSummary {
        benchmark: "list_perf",
        workload: workload.as_str(),
        total_ops: effective_total_ops,
        threads: thread_count,
        total_elapsed,
        hist: &combined_hist,
        db_size_bytes: final_db_size,
    };

    print_json_summary(&summary);
}
