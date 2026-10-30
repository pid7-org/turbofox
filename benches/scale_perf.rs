mod common;

use common::{BenchDir, BenchSummary, KEY_SIZE, VAL_SIZE, new_histogram, print_json_summary};
use std::time::Instant;
use turbofox::{EvictionMode, TurboCfg, TurboFox};

const TOTAL_KEYS: usize = 0x0001_F400;
const SAMPLES_PER_INTERVAL: usize = 0x10;
const BENCH_NAME: &str = "scale_perf";

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
    let (_bench_dir, db_path) = BenchDir::new(None, BENCH_NAME);
    let val = [0x42u8; VAL_SIZE];

    let num_intervals = total_keys / step;
    let mut current_keys = 0x00;
    let mut is_first = true;

    for interval_idx in 0x00..=num_intervals {
        let target_keys = interval_idx * step;

        if interval_idx == 0x00 {
            let create_cfg = TurboCfg::manual(&db_path);
            let writer = TurboFox::create(create_cfg).expect("failed to create db");

            writer.evict(EvictionMode::PurgeAll).expect("evict failed");
            drop(writer);
        } else {
            let open_cfg = TurboCfg::manual(&db_path);
            let writer = TurboFox::open(open_cfg).expect("reopen writer failed");

            for i in current_keys..target_keys {
                let key: [u8; KEY_SIZE] = (i as u32).to_be_bytes();
                writer.set(&key, &val).expect("insert failed");
            }

            current_keys = target_keys;
            writer.evict(EvictionMode::PurgeAll).expect("evict failed");

            drop(writer);
        }

        let base_cfg = TurboCfg::manual(&db_path);
        let disk_db = TurboFox::open(base_cfg).expect("disk probe open failed");
        let disk_bytes = disk_db.resident_bytes_on_disk() as u64;

        drop(disk_db);

        for &(lookup_cache, workload) in &[(false, "open_uncached"), (true, "open_cached")] {
            let turbo_cfg = TurboCfg::manual(&db_path).with_lookup_cache(lookup_cache);
            let mut hist = new_histogram();

            let warmup_db = TurboFox::open(turbo_cfg.clone()).expect("warmup open failed");
            let in_mem_bytes =
                warmup_db.resident_bytes_in_memory().expect("resident in-mem query failed") as u64;

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
                benchmark: BENCH_NAME,
                workload,
                total_ops: target_keys,
                threads: 0x01,
                total_elapsed,
                hist: &hist,
                db_size_bytes: disk_bytes,
                in_mem_bytes,
            };

            print_json_summary(&summary);
        }
    }
}
