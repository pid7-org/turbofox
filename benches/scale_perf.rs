mod common;

use common::{
    BenchDir, KEY_SIZE, NANOS_PER_MICRO, VAL_SIZE, pin_to_physical_core, purge_directory_page_cache,
};
use std::path::Path;
use std::time::Instant;
use turbofox::{EvictionMode, TurboCfg, TurboFox};

const TOTAL_KEYS: usize = 0x8000;
const INTERVALS: usize = 0x08;
const STEP: usize = TOTAL_KEYS / INTERVALS;
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

#[inline(always)]
fn measure_single_open(cfg: TurboCfg) -> (f64, u64, u64) {
    let t0 = Instant::now();
    let db = TurboFox::open(cfg).expect("open failed");
    let lat = (t0.elapsed().as_nanos() as f64) / NANOS_PER_MICRO;

    let mem = db.resident_bytes_in_memory().unwrap_or(0x00) as u64;
    let disk = db.resident_bytes_on_disk() as u64;

    let _ = db.evict(EvictionMode::PurgeAll);
    drop(db);

    (lat, mem, disk)
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
        let uncached_cfg = TurboCfg::manual(db_path).with_lookup_cache(false);

        // Alternate measurement order to neutralize systemic cache biasing
        let (uncached_lat, uncached_mem, uncached_disk, cached_lat, cached_mem, cached_disk) =
            if interval_idx % 0x02 == 0x00 {
                let (u_lat, u_mem, u_disk) = measure_single_open(uncached_cfg);
                purge_directory_page_cache(db_path);
                let (c_lat, c_mem, c_disk) = measure_single_open(cached_cfg);
                purge_directory_page_cache(db_path);
                (u_lat, u_mem, u_disk, c_lat, c_mem, c_disk)
            } else {
                let (c_lat, c_mem, c_disk) = measure_single_open(cached_cfg);
                purge_directory_page_cache(db_path);
                let (u_lat, u_mem, u_disk) = measure_single_open(uncached_cfg);
                purge_directory_page_cache(db_path);
                (u_lat, u_mem, u_disk, c_lat, c_mem, c_disk)
            };

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

    let (_bench_dir, db_path) = BenchDir::new(None, "scale_perf");
    let val = [0x42u8; VAL_SIZE];

    run_bench(&db_path, INTERVALS, STEP, val);
}
