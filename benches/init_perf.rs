mod common;

use common::{
    BenchConfig, BenchDir, BenchSummary, HIT_PREFIX, make_key, make_val, new_histogram,
    print_json_summary,
};
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

fn main() {
    let cfg = BenchConfig::parse();
    let scales = if std::env::args().any(|a| a == "--ops") {
        vec![cfg.ops]
    } else if !cfg.scales.is_empty() {
        cfg.scales.clone()
    } else {
        vec![cfg.ops]
    };

    let total_scales = scales.len();
    for (idx, &n) in scales.iter().enumerate() {
        let tag = format!("init_{}_{}", n, idx);
        let (bench_dir, db_path) = BenchDir::new(cfg.db_path.clone(), &tag);

        let turbo_cfg = TurboCfg::manual(&db_path).with_mem_index(cfg.mem_index);
        let db = TurboFox::create(turbo_cfg.clone()).expect("failed to create db");
        let val = make_val(0x42);

        for i in 0x00..n {
            let key = make_key(HIT_PREFIX, 0x00, i as u32);
            db.set(&key, &val).expect("insert failed");
        }
        db.sync().expect("sync failed");
        drop(db);

        let db_size = bench_dir.compute_disk_bytes();
        let samples = cfg.samples.max(0x0A);
        let mut hist = new_histogram();

        // Warmup single open
        let warmup_db = TurboFox::open(turbo_cfg.clone()).expect("warmup open failed");
        drop(warmup_db);

        let start_all = Instant::now();
        for _ in 0x00..samples {
            let t0 = Instant::now();
            let opened_db = TurboFox::open(turbo_cfg.clone()).expect("open failed");
            let elapsed_ns = t0.elapsed().as_nanos() as u64;

            hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            drop(opened_db);
        }
        let total_elapsed = start_all.elapsed();

        let workload_name = format!("scale_{}", n);
        let summary = BenchSummary {
            benchmark: "init_perf",
            workload: &workload_name,
            total_ops: n,
            threads: 0x01,
            total_elapsed,
            hist: &hist,
            db_size_bytes: db_size,
        };

        print_json_summary(&summary);
        if idx + 0x01 < total_scales {
            println!(",");
        }
    }
}
