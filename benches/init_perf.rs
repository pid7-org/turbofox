mod common;

use common::{BenchDir, BenchSummary, KEY_SIZE, VAL_SIZE, new_histogram, print_json_summary};
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

const OPS: usize = 0x186A0;
const SAMPLES: usize = 0x0100;

fn main() {
    let (bench_dir, db_path) = BenchDir::new(None, "init_perf");

    // Populate dataset once
    let create_cfg = TurboCfg::manual(&db_path);
    let db = TurboFox::create(create_cfg).expect("failed to create db");
    let val = [0x42u8; VAL_SIZE];

    for i in 0x00..OPS {
        let key: [u8; KEY_SIZE] = (i as u32).to_be_bytes();
        db.set(&key, &val).expect("insert failed");
    }

    db.sync().expect("sync failed");
    drop(db);

    let db_size = bench_dir.compute_disk_bytes();
    let configs = [("without_mem_index", false), ("with_mem_index", true)];
    let total_configs = configs.len();

    for (idx, &(workload, mem_index)) in configs.iter().enumerate() {
        let turbo_cfg = TurboCfg::manual(&db_path).with_mem_index(mem_index);
        let mut hist = new_histogram();

        // Warmup single open
        let warmup_db = TurboFox::open(turbo_cfg.clone()).expect("warmup open failed");
        drop(warmup_db);

        let start_all = Instant::now();
        for _ in 0x00..SAMPLES {
            let t0 = Instant::now();
            let opened_db = TurboFox::open(turbo_cfg.clone()).expect("open failed");
            let elapsed_ns = t0.elapsed().as_nanos() as u64;

            hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            drop(opened_db);
        }
        let total_elapsed = start_all.elapsed();

        let summary = BenchSummary {
            benchmark: "init_perf",
            workload,
            total_ops: OPS,
            threads: 0x01,
            total_elapsed,
            hist: &hist,
            db_size_bytes: db_size,
        };

        print_json_summary(&summary);
        if idx + 0x01 < total_configs {
            println!(",");
        }
    }
}
