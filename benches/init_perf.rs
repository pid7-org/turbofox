#![cfg(target_os = "linux")]

#[cfg(not(target_os = "linux"))]
compile_error!("benchmarks require target_os = \"linux\"");

mod common;

use common::{BenchDir, BenchSummary, KEY_SIZE, VAL_SIZE, new_histogram, print_json_summary};
use std::time::Instant;
use turbofox::{TurboCfg, TurboFox};

const TOTAL_KEYS: usize = 0x0001_86A0;
const SAMPLES_PER_INTERVAL: usize = 0x10;

fn main() {
    let total_keys = std::env::var("TURBOFOX_BENCH_OPS")
        .ok()
        .and_then(|v| {
            if v.starts_with("0x") || v.starts_with("0X") {
                usize::from_str_radix(&v[0x02..], 0x10).ok()
            } else {
                v.parse().ok()
            }
        })
        .unwrap_or(TOTAL_KEYS);

    let step = (total_keys / 0x0A).max(0x01);
    let (bench_dir, db_path) = BenchDir::new(None, "init_perf");
    let val = [0x42u8; VAL_SIZE];

    let num_intervals = total_keys / step;
    let mut current_keys = 0x00;
    let mut is_first = true;

    for interval_idx in 0x01..=num_intervals {
        let target_keys = interval_idx * step;

        let writer = if interval_idx == 0x01 {
            let create_cfg = TurboCfg::manual(&db_path);
            TurboFox::create(create_cfg).expect("failed to create db")
        } else {
            let open_cfg = TurboCfg::manual(&db_path);
            TurboFox::open(open_cfg).expect("reopen writer failed")
        };

        for i in current_keys..target_keys {
            let key: [u8; KEY_SIZE] = (i as u32).to_be_bytes();
            writer.set(&key, &val).expect("insert failed");
        }
        current_keys = target_keys;

        writer.sync().expect("sync failed");
        drop(writer);

        let db_size = bench_dir.compute_disk_bytes();

        let turbo_cfg = TurboCfg::manual(&db_path);
        let mut hist = new_histogram();

        let warmup_db = TurboFox::open(turbo_cfg.clone()).expect("warmup open failed");
        drop(warmup_db);

        let start_all = Instant::now();
        for _ in 0x00..SAMPLES_PER_INTERVAL {
            let t0 = Instant::now();
            let opened_db = TurboFox::open(turbo_cfg.clone()).expect("open failed");
            let elapsed_ns = t0.elapsed().as_nanos() as u64;

            hist.record(elapsed_ns.max(0x01)).expect("hist record failed");
            drop(opened_db);
        }
        let total_elapsed = start_all.elapsed();

        if !is_first {
            println!(",");
        }
        is_first = false;

        let summary = BenchSummary {
            benchmark: "init_perf",
            workload: "open",
            total_ops: target_keys,
            threads: 0x01,
            total_elapsed,
            hist: &hist,
            db_size_bytes: db_size,
        };

        print_json_summary(&summary);
    }
}
