mod common;

use common::{
    BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, VAL_SIZE, generate_60_40_probe_keys,
    generate_del_60_40_keys, make_key, make_val, new_histogram, pin_to_physical_core,
    print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{CapacityHint, TurboCfg, TurboFox};

const TOTAL_OPS: usize = 0x0008_0000;
const THREAD_CONFIGS: [usize; 0x02] = [0x01, 0x02];

fn generate_set_80_20_keys(thread_id: usize, count: usize) -> Vec<[u8; KEY_SIZE]> {
    let new_target = (count * 0x50) / 0x64;
    let mut new_count = 0x00;
    let mut update_count = 0x00;
    let mut keys = Vec::with_capacity(count);

    for i in 0x00..count {
        let is_new = if new_count < new_target {
            let remaining_total = count - i;
            let remaining_news = new_target - new_count;

            (i * 0x50) % 0x64 < 0x50 || remaining_news >= remaining_total
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

#[inline(always)]
fn calculate_capacity_hint_ops(total_ops: usize) -> usize {
    (total_ops * 0x7D) / 0x64
}

fn run_tier(thread_count: usize, total_ops: usize, is_first: &mut bool) {
    let (bench_dir, db_path) = BenchDir::new(None, "kv_perf");

    let hint_ops = calculate_capacity_hint_ops(total_ops);
    let hint = CapacityHint::new().with_kv(hint_ops, KEY_SIZE, VAL_SIZE);
    let turbo_cfg = TurboCfg::manual(&db_path).with_capacity_hint(hint);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let ops_per_thread = total_ops / thread_count;

    // -------------------------------------------------------------
    // (P1) SET (80/20 New/Update)
    // -------------------------------------------------------------
    let mut set_keys = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        set_keys.push(generate_set_80_20_keys(tid, ops_per_thread));
    }

    let barrier_set = Arc::new(Barrier::new(thread_count));
    let mut handles_set = Vec::with_capacity(thread_count);

    let start_set = Instant::now();
    for (tid, keys) in set_keys.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_set);

        handles_set.push(thread::spawn(move || {
            pin_to_physical_core(tid);
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

    // -------------------------------------------------------------
    // (P2) GET (Mixed 60/40 Hit/Miss)
    // -------------------------------------------------------------
    let mut get_probes = Vec::with_capacity(thread_count);

    let hit_pool_size = (ops_per_thread * 0x3C) / 0x64;
    for tid in 0x00..thread_count {
        get_probes.push(generate_60_40_probe_keys(tid, ops_per_thread, hit_pool_size));
    }

    let barrier_get = Arc::new(Barrier::new(thread_count));
    let mut handles_get = Vec::with_capacity(thread_count);

    let start_get = Instant::now();
    for (tid, probes) in get_probes.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_get);

        handles_get.push(thread::spawn(move || {
            pin_to_physical_core(tid);
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

    let mut get_hist = new_histogram();
    for h in handles_get {
        let th = h.join().expect("get thread panicked");
        get_hist.add(&th).expect("failed to merge get hist");
    }

    let elapsed_get = start_get.elapsed();
    let db_size_get = bench_dir.compute_disk_bytes();

    // -------------------------------------------------------------
    // (P3) DEL (Mixed 60/40 Hit/Miss)
    // -------------------------------------------------------------
    let mut del_keys = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        let (keys, _) = generate_del_60_40_keys(tid, ops_per_thread);
        del_keys.push(keys);
    }

    let barrier_del = Arc::new(Barrier::new(thread_count));
    let mut handles_del = Vec::with_capacity(thread_count);

    let start_del = Instant::now();
    for (tid, keys) in del_keys.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_del);

        handles_del.push(thread::spawn(move || {
            pin_to_physical_core(tid);
            let mut hist = new_histogram();
            bar.wait();

            for key in &keys {
                let t0 = Instant::now();
                let mut attempts = 0x00;
                let res = loop {
                    match db_clone.del(key) {
                        Ok(r) => break r,
                        Err(turbofox::TurboError::ResourceBusy(_)) => {
                            attempts += 0x01;
                            if attempts >= 0x64 {
                                panic!("del failed: max OCC retries reached");
                            }

                            thread::yield_now();
                        }
                        Err(e) => panic!("del failed: {e:?}"),
                    }
                };

                std::hint::black_box(res);
                let elapsed_ns = t0.elapsed().as_nanos() as u64;

                hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            }

            hist
        }));
    }

    let mut del_hist = new_histogram();
    for h in handles_del {
        let th = h.join().expect("del thread panicked");
        del_hist.add(&th).expect("failed to merge del hist");
    }

    let elapsed_del = start_del.elapsed();
    let db_size_del = bench_dir.compute_disk_bytes();

    // -------------------------------------------------------------
    // Output JSON entries for SET, GET, and DEL
    // -------------------------------------------------------------
    let summary_set = BenchSummary {
        benchmark: "set_perf",
        workload: "mixed_80_20_new_update",
        total_ops,
        threads: thread_count,
        total_elapsed: elapsed_set,
        hist: &set_hist,
        db_size_bytes: db_size_set,
    };
    if !*is_first {
        println!(",");
    }
    *is_first = false;
    print_json_summary(&summary_set);

    println!(",");
    let summary_get = BenchSummary {
        benchmark: "get_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: thread_count,
        total_elapsed: elapsed_get,
        hist: &get_hist,
        db_size_bytes: db_size_get,
    };
    print_json_summary(&summary_get);

    println!(",");
    let summary_del = BenchSummary {
        benchmark: "del_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: thread_count,
        total_elapsed: elapsed_del,
        hist: &del_hist,
        db_size_bytes: db_size_del,
    };
    print_json_summary(&summary_del);
}

fn main() {
    let total_ops = std::env::var("TURBOFOX_BENCH_OPS")
        .ok()
        .and_then(|v| {
            if v.starts_with("0x") || v.starts_with("0X") {
                usize::from_str_radix(&v[0x02..], 0x10).ok()
            } else {
                v.parse().ok()
            }
        })
        .unwrap_or(TOTAL_OPS);

    let mut is_first = true;
    for &threads in &THREAD_CONFIGS {
        run_tier(threads, total_ops, &mut is_first);
    }
}
