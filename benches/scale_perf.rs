mod common;

use common::{
    BenchDir, KEY_SIZE, NANOS_PER_MICRO, VAL_SIZE, pin_to_physical_core, purge_directory_page_cache,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Barrier;
use std::thread;
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

#[inline(always)]
fn run_worker(
    tid: usize,
    db_path: &Path,
    lookup_cache: bool,
    workload: &'static str,
    num_intervals: usize,
    step: usize,
    val: [u8; VAL_SIZE],
    barrier: &Barrier,
) {
    pin_to_physical_core(tid);

    let create_cfg = TurboCfg::manual(db_path);
    let writer = TurboFox::create(create_cfg).expect("failed to create db");
    writer.evict(EvictionMode::PurgeAll).expect("evict failed");

    drop(writer);
    purge_directory_page_cache(db_path);

    barrier.wait();

    let mut current_keys = 0x00;
    for interval_idx in 0x00..=num_intervals {
        let cfg = TurboCfg::manual(db_path).with_lookup_cache(lookup_cache);
        let t0 = Instant::now();
        let opened_db = TurboFox::open(cfg).expect("open failed");
        let latency_micros = (t0.elapsed().as_nanos() as f64) / NANOS_PER_MICRO;
        let in_mem = opened_db.resident_bytes_in_memory().expect("in-mem query failed") as u64;
        let disk = opened_db.resident_bytes_on_disk() as u64;
        drop(opened_db);

        barrier.wait();

        if tid == 0x00 {
            if interval_idx > 0x00 {
                println!(",");
            }

            print_scale_json(workload, current_keys, disk, in_mem, latency_micros);
        }

        barrier.wait();

        if tid == 0x01 {
            println!(",");
            print_scale_json(workload, current_keys, disk, in_mem, latency_micros);
        }

        barrier.wait();

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

            barrier.wait();

            purge_directory_page_cache(db_path);

            barrier.wait();
        }
    }
}

fn main() {
    let total_keys = std::env::var("TURBOFOX_BENCH_OPS")
        .ok()
        .and_then(|v| {
            if v.starts_with("0x") {
                usize::from_str_radix(&v[0x02..], 0x10).ok()
            } else {
                v.parse().ok()
            }
        })
        .unwrap_or(TOTAL_KEYS);

    let bench_dir_path = PathBuf::from("./tmp/db");
    let _ = fs::create_dir_all(&bench_dir_path);

    let step = (total_keys / 0x0A).max(0x01);
    let (_bench_dir1, db1_path) =
        BenchDir::new(Some(bench_dir_path.clone()), "scale_perf_uncached");
    let (_bench_dir2, db2_path) = BenchDir::new(Some(bench_dir_path), "scale_perf_cached");

    let val = [0x42u8; VAL_SIZE];
    let num_intervals = total_keys / step;
    let barrier = Barrier::new(0x02);

    thread::scope(|s| {
        let t1 = s.spawn(|| {
            run_worker(
                0x00,
                &db1_path,
                false,
                "lookup_cache_disabled",
                num_intervals,
                step,
                val,
                &barrier,
            );
        });

        let t2 = s.spawn(|| {
            run_worker(
                0x01,
                &db2_path,
                true,
                "lookup_cache_enabled",
                num_intervals,
                step,
                val,
                &barrier,
            );
        });

        t1.join().expect("uncached worker thread panicked");
        t2.join().expect("cached worker thread panicked");
    });
}
