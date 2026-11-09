mod common;

use common::{
    BenchDir, BenchSummary, KEY_SIZE, LATENCY_SAMPLE_MASK, VAL_SIZE, generate_60_40_probe_keys,
    generate_del_60_40_keys, generate_set_80_20_keys, make_key, make_val, new_histogram,
    pin_to_physical_core, print_json_summary, spin_backoff,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{CapacityHint, TurboCfg, TurboFox};

const TOTAL_OPS: usize = 0x0010_0000;
const THREAD_COUNT: usize = 0x02;
const WARMUP_OPS: usize = 0x0200;

fn untimed_warmup_worker(db: &TurboFox, tid: usize) {
    for i in 0x00..WARMUP_OPS {
        let key = make_key(0x7FFE, tid as u16, i as u32);
        let val = make_val(i as u32);
        let _ = db.set(&key, &val);
        let _ = db.get(&key);
        let _ = db.del(&key);
    }
}

fn run_multi_thread(total_ops: usize) {
    let (_bench_dir, db_path) = BenchDir::new(None, "kv_perf_multi_tx");

    let hint_ops = total_ops * 0x02;
    let hint = CapacityHint::new().with_kv(hint_ops, KEY_SIZE, VAL_SIZE);
    let turbo_cfg = TurboCfg::manual(&db_path).with_capacity_hint(hint).with_lookup_cache(true);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let ops_per_thread = total_ops / THREAD_COUNT;
    let hit_pool_size = (ops_per_thread * 0x3C) / 0x64;

    // Pre-allocate all datasets for all worker threads upfront
    let mut set_keys_all = Vec::with_capacity(THREAD_COUNT);
    let mut get_probes_all = Vec::with_capacity(THREAD_COUNT);
    let mut del_keys_all = Vec::with_capacity(THREAD_COUNT);

    for tid in 0x00..THREAD_COUNT {
        set_keys_all.push(generate_set_80_20_keys(tid, ops_per_thread));
        get_probes_all.push(generate_60_40_probe_keys(tid, ops_per_thread, hit_pool_size));
        let (keys, _) = generate_del_60_40_keys(tid, ops_per_thread);
        del_keys_all.push(keys);
    }

    // -------------------------------------------------------------
    // (P1) CONCURRENT SET (80/20 New/Update across 2 threads)
    // -------------------------------------------------------------
    let barrier_set = Arc::new(Barrier::new(THREAD_COUNT + 0x01));
    let mut handles_set = Vec::with_capacity(THREAD_COUNT);

    for (tid, keys) in set_keys_all.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_set);

        handles_set.push(thread::spawn(move || {
            pin_to_physical_core(tid);
            untimed_warmup_worker(&db_clone, tid);
            let mut hist = new_histogram();

            bar.wait();

            for (op_idx, key) in keys.iter().enumerate() {
                let val: [u8; VAL_SIZE] = make_val(((tid as u32) << 0x10) | (op_idx as u32));
                let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
                let t0 = if sample { Some(Instant::now()) } else { None };

                let mut attempts = 0x00;
                let ticket = loop {
                    match db_clone.set(key, &val) {
                        Ok(t) => break t,
                        Err(turbofox::TurboError::ResourceBusy(_)) => {
                            attempts += 0x01;
                            if attempts >= 0x0400 {
                                panic!("set failed: max OCC retries reached");
                            }
                            spin_backoff(attempts);
                        }
                        Err(e) => panic!("set failed: {e:?}"),
                    }
                };

                std::hint::black_box(&ticket);
                if let Some(t) = t0 {
                    let elapsed_ns = t.elapsed().as_nanos() as u64;
                    hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
                }
            }

            hist
        }));
    }

    barrier_set.wait();
    let start_set = Instant::now();

    let mut set_hist = new_histogram();
    for h in handles_set {
        let th = h.join().expect("set thread panicked");
        set_hist.add(&th).expect("failed to merge set hist");
    }
    let elapsed_set = start_set.elapsed();

    db.sync().expect("sync after set failed");
    let db_size_set = db.resident_bytes_on_disk() as u64;
    let in_mem_set = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // -------------------------------------------------------------
    // (P2) CONCURRENT GET (60/40 Hit/Miss on same instance)
    // -------------------------------------------------------------
    let barrier_get = Arc::new(Barrier::new(THREAD_COUNT + 0x01));
    let mut handles_get = Vec::with_capacity(THREAD_COUNT);

    for (tid, probes) in get_probes_all.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_get);

        handles_get.push(thread::spawn(move || {
            pin_to_physical_core(tid);
            let mut hist = new_histogram();

            bar.wait();

            for (op_idx, key) in probes.iter().enumerate() {
                let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
                let t0 = if sample { Some(Instant::now()) } else { None };
                let res = db_clone.get(key).expect("get failed");

                std::hint::black_box(&res);
                if let Some(t) = t0 {
                    let elapsed_ns = t.elapsed().as_nanos() as u64;
                    hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
                }
            }

            hist
        }));
    }

    barrier_get.wait();
    let start_get = Instant::now();

    let mut get_hist = new_histogram();
    for h in handles_get {
        let th = h.join().expect("get thread panicked");
        get_hist.add(&th).expect("failed to merge get hist");
    }
    let elapsed_get = start_get.elapsed();

    let db_size_get = db.resident_bytes_on_disk() as u64;
    let in_mem_get = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // -------------------------------------------------------------
    // (P3) CONCURRENT DEL (60/40 Hit/Miss on same instance)
    // -------------------------------------------------------------
    let barrier_del = Arc::new(Barrier::new(THREAD_COUNT + 0x01));
    let mut handles_del = Vec::with_capacity(THREAD_COUNT);

    for (tid, keys) in del_keys_all.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_del);

        handles_del.push(thread::spawn(move || {
            pin_to_physical_core(tid);
            let mut hist = new_histogram();

            bar.wait();

            for (op_idx, key) in keys.iter().enumerate() {
                let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
                let t0 = if sample { Some(Instant::now()) } else { None };
                let mut attempts = 0x00;
                let res = loop {
                    match db_clone.del(key) {
                        Ok(r) => break r,
                        Err(turbofox::TurboError::ResourceBusy(_)) => {
                            attempts += 0x01;
                            if attempts >= 0x0400 {
                                panic!("del failed: max OCC retries reached");
                            }
                            spin_backoff(attempts);
                        }
                        Err(e) => panic!("del failed: {e:?}"),
                    }
                };

                std::hint::black_box(res);
                if let Some(t) = t0 {
                    let elapsed_ns = t.elapsed().as_nanos() as u64;
                    hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
                }
            }

            hist
        }));
    }

    barrier_del.wait();
    let start_del = Instant::now();

    let mut del_hist = new_histogram();
    for h in handles_del {
        let th = h.join().expect("del thread panicked");
        del_hist.add(&th).expect("failed to merge del hist");
    }
    let elapsed_del = start_del.elapsed();

    let db_size_del = db.resident_bytes_on_disk() as u64;
    let in_mem_del = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // Output JSON summaries
    let summary_set = BenchSummary {
        benchmark: "set_perf_multi_tx",
        workload: "mixed_80_20_new_update",
        total_ops,
        threads: THREAD_COUNT,
        total_elapsed: elapsed_set,
        hist: &set_hist,
        db_size_bytes: db_size_set,
        in_mem_bytes: in_mem_set,
    };
    print_json_summary(&summary_set);

    println!(",");
    let summary_get = BenchSummary {
        benchmark: "get_perf_multi_tx",
        workload: "mixed_60_40",
        total_ops,
        threads: THREAD_COUNT,
        total_elapsed: elapsed_get,
        hist: &get_hist,
        db_size_bytes: db_size_get,
        in_mem_bytes: in_mem_get,
    };
    print_json_summary(&summary_get);

    println!(",");
    let summary_del = BenchSummary {
        benchmark: "del_perf_multi_tx",
        workload: "mixed_60_40",
        total_ops,
        threads: THREAD_COUNT,
        total_elapsed: elapsed_del,
        hist: &del_hist,
        db_size_bytes: db_size_del,
        in_mem_bytes: in_mem_del,
    };
    print_json_summary(&summary_del);
}

fn main() {
    run_multi_thread(TOTAL_OPS);
}
