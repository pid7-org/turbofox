mod common;

use common::{
    BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, VAL_SIZE, make_key, make_val, new_histogram,
    print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

const TOTAL_OPS: usize = 0x0004_0000;
const THREAD_COUNT: usize = 0x02;
const OPS_PER_THREAD: usize = TOTAL_OPS / THREAD_COUNT;

fn generate_set_60_40_keys(thread_id: usize, count: usize) -> Vec<[u8; KEY_SIZE]> {
    let new_target = (count * 0x3C) / 0x64;
    let mut new_count = 0x00;
    let mut update_count = 0x00;
    let mut keys = Vec::with_capacity(count);

    for i in 0x00..count {
        let is_new = if new_count < new_target {
            let remaining_total = count - i;
            let remaining_news = new_target - new_count;
            (i * 0x3C) % 0x64 < 0x3C || remaining_news >= remaining_total
        } else {
            false
        };

        if (is_new || new_count == 0x00) && new_count < new_target {
            keys.push(make_key(HIT_PREFIX, thread_id as u16, new_count as u32));
            new_count += 0x01;
        } else {
            let target_id = (update_count % new_count) as u32;
            keys.push(make_key(HIT_PREFIX, thread_id as u16, target_id));
            update_count += 0x01;
        }
    }

    keys
}

fn main() {
    let (bench_dir, db_path) = BenchDir::new(None, "kv_perf");

    let turbo_cfg = TurboCfg::manual(&db_path);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let mut set_keys = Vec::with_capacity(THREAD_COUNT);
    for tid in 0x00..THREAD_COUNT {
        set_keys.push(generate_set_60_40_keys(tid, OPS_PER_THREAD));
    }

    let barrier_set = Arc::new(Barrier::new(THREAD_COUNT));
    let mut handles_set = Vec::with_capacity(THREAD_COUNT);
    let start_set = Instant::now();

    for (tid, keys) in set_keys.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_set);
        handles_set.push(thread::spawn(move || {
            let mut hist = new_histogram();
            bar.wait();

            for (op_idx, key) in keys.iter().enumerate() {
                let val: [u8; VAL_SIZE] = make_val(((tid as u32) << 0x10) | (op_idx as u32));
                let t0 = Instant::now();
                let mut attempts = 0x00;
                let ticket = loop {
                    match db_clone.set(key, &val) {
                        Ok(t) => break t,
                        Err(turbofox::TurboError::ResourceBusy(_)) => {
                            attempts += 0x01;
                            if attempts >= 0x64 {
                                panic!("set failed: max OCC retries reached");
                            }
                            thread::yield_now();
                        }
                        Err(e) => panic!("set failed: {e:?}"),
                    }
                };

                std::hint::black_box(&ticket);
                let elapsed_ns = t0.elapsed().as_nanos() as u64;

                hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            }

            hist
        }));
    }

    let mut set_hist = new_histogram();
    for h in handles_set {
        let th = h.join().expect("set thread panicked");
        set_hist.add(&th).expect("failed to merge set hist");
    }
    let elapsed_set = start_set.elapsed();
    let db_size_set = bench_dir.compute_disk_bytes();

    db.sync().expect("sync after set failed");

    let summary_set = BenchSummary {
        benchmark: "set_perf",
        workload: "mixed_60_40_new_update",
        total_ops: TOTAL_OPS,
        threads: THREAD_COUNT,
        total_elapsed: elapsed_set,
        hist: &set_hist,
        db_size_bytes: db_size_set,
    };
    print_json_summary(&summary_set);
}
