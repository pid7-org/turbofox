mod common;

use common::{
    BenchDir, KEY_SIZE, NANOS_PER_MICRO, VAL_SIZE, pin_to_physical_core, purge_directory_page_cache,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use turbofox::{EvictionMode, TurboCfg, TurboFox};

const TOTAL_KEYS: usize = 0x0001_F400;
const BENCH_NAME: &str = "scale_perf";

#[inline(always)]
fn print_scale_json(
    workload: &str,
    operations: usize,
    db_size_bytes: u64,
    in_mem_bytes: u64,
    latency_micros: f64,
) {
    println!("{{");
    println!("  \"benchmark\": \"{}\",", BENCH_NAME);
    println!("  \"workload\": \"{}\",", workload);
    println!("  \"operations\": {},", operations);
    println!("  \"threads\": 1,");
    println!("  \"db_size_bytes\": {},", db_size_bytes);
    println!("  \"in_mem_bytes\": {},", in_mem_bytes);
    println!("  \"latency_micros\": {:.2}", latency_micros);
    println!("}}");
}

fn run_bench(db_path: &Path, num_intervals: usize, step: usize, val: [u8; VAL_SIZE]) {
    let create_cfg = TurboCfg::manual(db_path);
    let writer = TurboFox::create(create_cfg).expect("failed to create db");
    writer.evict(EvictionMode::PurgeAll).expect("evict failed");
    drop(writer);
    purge_directory_page_cache(db_path);

    // Untimed warmup to prime dynamic loader, CPU governor, and memory mapping paths
    {
        let warmup_cfg = TurboCfg::manual(db_path).with_lookup_cache(false);
        if let Ok(warmup_db) = TurboFox::open(warmup_cfg) {
            let _ = warmup_db.resident_bytes_in_memory();
            let _ = warmup_db.evict(EvictionMode::PurgeAll);

            drop(warmup_db);
        }

        let warmup_cached = TurboCfg::manual(db_path).with_lookup_cache(true);
        if let Ok(warmup_db) = TurboFox::open(warmup_cached) {
            let _ = warmup_db.resident_bytes_in_memory();
            let _ = warmup_db.evict(EvictionMode::PurgeAll);

            drop(warmup_db);
        }

        purge_directory_page_cache(db_path);
    }

    let mut current_keys = 0x00;
    for interval_idx in 0x00..=num_intervals {
        let cached_cfg = TurboCfg::manual(db_path).with_lookup_cache(true);
        let t1 = Instant::now();
        let cached_db = TurboFox::open(cached_cfg).expect("open cached failed");
        let cached_lat = (t1.elapsed().as_nanos() as f64) / NANOS_PER_MICRO;
        let cached_mem = cached_db.resident_bytes_in_memory().expect("in-mem query failed") as u64;
        let cached_disk = cached_db.resident_bytes_on_disk() as u64;
        cached_db.evict(EvictionMode::PurgeAll).expect("evict failed");
        drop(cached_db);
        purge_directory_page_cache(db_path);

        let uncached_cfg = TurboCfg::manual(db_path).with_lookup_cache(false);
        let t0 = Instant::now();
        let uncached_db = TurboFox::open(uncached_cfg).expect("open uncached failed");
        let uncached_lat = (t0.elapsed().as_nanos() as f64) / NANOS_PER_MICRO;
        let uncached_mem =
            uncached_db.resident_bytes_in_memory().expect("in-mem query failed") as u64;
        let uncached_disk = uncached_db.resident_bytes_on_disk() as u64;
        uncached_db.evict(EvictionMode::PurgeAll).expect("evict failed");
        drop(uncached_db);
        purge_directory_page_cache(db_path);

        if interval_idx > 0x00 {
            println!(",");
        }
        print_scale_json(
            "lookup_cache_disabled",
            current_keys,
            uncached_disk,
            uncached_mem,
            uncached_lat,
        );
        println!(",");
        print_scale_json("lookup_cache_enabled", current_keys, cached_disk, cached_mem, cached_lat);

        if interval_idx < num_intervals {
            let target_keys = (interval_idx + 0x01) * step;
            let writer_cfg = TurboCfg::manual(db_path);
            let writer = TurboFox::open(writer_cfg).expect("reopen writer failed");

            for i in current_keys..target_keys {
                let key: [u8; KEY_SIZE] = (i as u32).to_be_bytes();
                writer.set(&key, &val).expect("insert failed");
            }

            current_keys = target_keys;
            writer.evict(EvictionMode::PurgeAll).expect("evict failed");
            drop(writer);

            purge_directory_page_cache(db_path);
        }
    }
}

fn main() {
    pin_to_physical_core(0x00);

    let total_keys = std::env::var("TURBOFOX_BENCH_OPS")
        .ok()
        .and_then(|v| {
            if let Some(stripped) = v.strip_prefix("0x") {
                usize::from_str_radix(stripped, 0x10).ok()
            } else {
                v.parse().ok()
            }
        })
        .unwrap_or(TOTAL_KEYS);

    let bench_dir_path = PathBuf::from("./tmp/db");
    let _ = fs::create_dir_all(&bench_dir_path);

    let step = (total_keys / 0x0A).max(0x01);
    let (_bench_dir, db_path) = BenchDir::new(Some(bench_dir_path), "scale_perf");

    let val = [0x42u8; VAL_SIZE];
    let num_intervals = total_keys / step;

    run_bench(&db_path, num_intervals, step, val);
}
