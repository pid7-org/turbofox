mod common;

use common::{
    BenchDir, BenchSummary, KEY_SIZE, LATENCY_SAMPLE_MASK, VAL_SIZE, generate_60_40_probe_keys,
    generate_del_60_40_keys, generate_set_80_20_keys, make_key, make_val, new_histogram,
    pin_to_physical_core, print_json_summary, spin_backoff,
};
use std::time::Instant;
use turbofox::{CapacityHint, TurboCfg, TurboFox};

const TOTAL_OPS: usize = 0x0010_0000;
const WARMUP_OPS: usize = 0x0400;

fn untimed_warmup(db: &TurboFox) {
    for i in 0x00..WARMUP_OPS {
        let key = make_key(0x7FFF, 0x00, i as u32);
        let val = make_val(i as u32);
        let _ = db.set(&key, &val);
        let _ = db.get(&key);
        let _ = db.del(&key);
    }
    let _ = db.sync();
}

fn run_single_thread(total_ops: usize) {
    pin_to_physical_core(0x00);

    let (_bench_dir, db_path) = BenchDir::new(None, "kv_perf");

    let hint_ops = total_ops * 0x02;
    let hint = CapacityHint::new().with_kv(hint_ops, KEY_SIZE, VAL_SIZE);
    let turbo_cfg = TurboCfg::manual(&db_path).with_capacity_hint(hint).with_lookup_cache(true);
    let db = TurboFox::create(turbo_cfg).expect("failed to create db");

    untimed_warmup(&db);

    // Pre-allocate all datasets prior to timer activation
    let set_keys = generate_set_80_20_keys(0x00, total_ops);
    let hit_pool_size = (total_ops * 0x3C) / 0x64;
    let get_probes = generate_60_40_probe_keys(0x00, total_ops, hit_pool_size);
    let (del_keys, _) = generate_del_60_40_keys(0x00, total_ops);

    // -------------------------------------------------------------
    // (P1) SET (80/20 New/Update)
    // -------------------------------------------------------------
    let mut set_hist = new_histogram();
    let start_set = Instant::now();

    for (op_idx, key) in set_keys.iter().enumerate() {
        let val: [u8; VAL_SIZE] = make_val(op_idx as u32);
        let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
        let t0 = if sample { Some(Instant::now()) } else { None };

        let mut attempts = 0x00;
        let ticket = loop {
            match db.set(key, &val) {
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
            set_hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
        }
    }

    let elapsed_set = start_set.elapsed();
    db.sync().expect("sync after set failed");

    let db_size_set = db.resident_bytes_on_disk() as u64;
    let in_mem_set = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // -------------------------------------------------------------
    // (P2) GET (Mixed 60/40 Hit/Miss on same instance)
    // -------------------------------------------------------------
    let mut get_hist = new_histogram();
    let start_get = Instant::now();

    for (op_idx, key) in get_probes.iter().enumerate() {
        let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
        let t0 = if sample { Some(Instant::now()) } else { None };
        let res = db.get(key).expect("get failed");

        std::hint::black_box(&res);
        if let Some(t) = t0 {
            let elapsed_ns = t.elapsed().as_nanos() as u64;
            get_hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
        }
    }

    let elapsed_get = start_get.elapsed();
    let db_size_get = db.resident_bytes_on_disk() as u64;
    let in_mem_get = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // -------------------------------------------------------------
    // (P3) DEL (Mixed 60/40 Hit/Miss on same instance)
    // -------------------------------------------------------------
    let mut del_hist = new_histogram();
    let start_del = Instant::now();

    for (op_idx, key) in del_keys.iter().enumerate() {
        let sample = (op_idx & LATENCY_SAMPLE_MASK) == 0;
        let t0 = if sample { Some(Instant::now()) } else { None };
        let mut attempts = 0x00;
        let res = loop {
            match db.del(key) {
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
            del_hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
        }
    }

    let elapsed_del = start_del.elapsed();
    let db_size_del = db.resident_bytes_on_disk() as u64;
    let in_mem_del = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;

    // Output JSON summaries
    let summary_set = BenchSummary {
        benchmark: "set_perf",
        workload: "mixed_80_20_new_update",
        total_ops,
        threads: 0x01,
        total_elapsed: elapsed_set,
        hist: &set_hist,
        db_size_bytes: db_size_set,
        in_mem_bytes: in_mem_set,
    };
    print_json_summary(&summary_set);

    println!(",");
    let summary_get = BenchSummary {
        benchmark: "get_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: 0x01,
        total_elapsed: elapsed_get,
        hist: &get_hist,
        db_size_bytes: db_size_get,
        in_mem_bytes: in_mem_get,
    };
    print_json_summary(&summary_get);

    println!(",");
    let summary_del = BenchSummary {
        benchmark: "del_perf",
        workload: "mixed_60_40",
        total_ops,
        threads: 0x01,
        total_elapsed: elapsed_del,
        hist: &del_hist,
        db_size_bytes: db_size_del,
        in_mem_bytes: in_mem_del,
    };
    print_json_summary(&summary_del);
}

fn main() {
    let ops = std::env::var("BENCH_OPS")
        .or_else(|_| std::env::var("KV_OPS"))
        .ok()
        .and_then(|v| {
            let clean = v.trim().replace('_', "");
            if clean.starts_with("0x") || clean.starts_with("0X") {
                usize::from_str_radix(&clean[0x02..], 0x10).ok()
            } else {
                clean.parse::<usize>().ok()
            }
        })
        .unwrap_or(TOTAL_OPS);
    run_single_thread(ops);
}
