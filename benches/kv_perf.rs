mod common;

use common::{
    BenchConfig, BenchDir, BenchSummary, HIT_PREFIX, KEY_SIZE, VAL_SIZE, generate_60_40_probe_keys,
    generate_del_60_40_keys, make_key, make_val, new_histogram, print_json_summary,
};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn main() {
    let cfg = BenchConfig::parse();
    let (bench_dir, db_path) = BenchDir::new(cfg.db_path.clone(), "kv_perf");

    let turbo_cfg = TurboCfg::manual(&db_path);
    let db = Arc::new(TurboFox::create(turbo_cfg).expect("failed to create db"));

    let thread_count = cfg.threads.max(0x01);
    let ops_per_thread = cfg.ops / thread_count;
    let total_ops = ops_per_thread * thread_count;

    // -------------------------------------------------------------
    // Phase 1: SET (Insert)
    // -------------------------------------------------------------
    let mut set_keys = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        let mut keys: Vec<[u8; KEY_SIZE]> = Vec::with_capacity(ops_per_thread);
        for i in 0x00..ops_per_thread {
            keys.push(make_key(HIT_PREFIX, tid as u16, i as u32));
        }
        set_keys.push(keys);
    }

    let barrier_set = Arc::new(Barrier::new(thread_count));
    let mut handles_set = Vec::with_capacity(thread_count);
    let start_set = Instant::now();

    for (tid, keys) in set_keys.into_iter().enumerate() {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_set);
        let val: [u8; VAL_SIZE] = make_val(tid as u32);

        handles_set.push(thread::spawn(move || {
            let mut hist = new_histogram();
            bar.wait();

            for key in &keys {
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
                            std::thread::yield_now();
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

    // Persistence barrier: sync dirty pages before read phase
    db.sync().expect("sync after set failed");

    // -------------------------------------------------------------
    // Phase 2: GET (Mixed 60/40 Hit/Miss)
    // -------------------------------------------------------------
    let mut get_probes = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        get_probes.push(generate_60_40_probe_keys(tid, ops_per_thread, ops_per_thread));
    }

    let barrier_get = Arc::new(Barrier::new(thread_count));
    let mut handles_get = Vec::with_capacity(thread_count);
    let start_get = Instant::now();

    for probes in get_probes {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_get);

        handles_get.push(thread::spawn(move || {
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
    // Phase 3: DEL (Mixed 60/40 Hit/Miss)
    // -------------------------------------------------------------
    let mut del_keys = Vec::with_capacity(thread_count);
    for tid in 0x00..thread_count {
        let (keys, _) = generate_del_60_40_keys(tid, ops_per_thread);
        del_keys.push(keys);
    }

    let barrier_del = Arc::new(Barrier::new(thread_count));
    let mut handles_del = Vec::with_capacity(thread_count);
    let start_del = Instant::now();

    for keys in del_keys {
        let db_clone = Arc::clone(&db);
        let bar = Arc::clone(&barrier_del);

        handles_del.push(thread::spawn(move || {
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
                            std::thread::yield_now();
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
        workload: "insert",
        total_ops,
        threads: thread_count,
        total_elapsed: elapsed_set,
        hist: &set_hist,
        db_size_bytes: db_size_set,
    };
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
