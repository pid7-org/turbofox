use hdrhistogram::Histogram;
use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

const DEFAULT_OPS: usize = 0x186A0;
const DEFAULT_THREADS: usize = 0x04;
const KEY_SIZE: usize = 0x0C;
const VAL_SIZE: usize = 0x30;
const SIGFIG: u8 = 3;
const NANOS_PER_MICRO: f64 = 0x03E8 as f64;
const BYTES_PER_MIB: f64 = 0x0010_0000 as f64;
const HISTOGRAM_MAX_NANOS: u64 = 0x0DF8_4758_0000;

fn parse_cli_args() -> (usize, usize) {
    let mut ops = DEFAULT_OPS;
    let mut threads = DEFAULT_THREADS;
    let args: Vec<String> = std::env::args().collect();
    let mut idx = 1;
    while idx < args.len() {
        if args[idx] == "--ops" && idx + 1 < args.len() {
            let val_str = &args[idx + 1];
            if val_str.starts_with("0x") || val_str.starts_with("0X") {
                if let Ok(v) = usize::from_str_radix(&val_str[2..], 0x10) {
                    ops = v;
                }
            } else if let Ok(v) = val_str.parse::<usize>() {
                ops = v;
            }
            idx += 2;
        } else if args[idx] == "--threads" && idx + 1 < args.len() {
            let val_str = &args[idx + 1];
            if let Ok(v) = val_str.parse::<usize>() {
                threads = v;
            }
            idx += 2;
        } else {
            idx += 1;
        }
    }
    (ops, threads)
}

fn pregenerate_keys(thread_id: usize, count: usize) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    for i in 0..count {
        let mut key = [0u8; KEY_SIZE];
        key[0..0x04].copy_from_slice(&(thread_id as u32).to_be_bytes());
        key[0x04..0x08].copy_from_slice(&(i as u32).to_be_bytes());
        keys.push(key);
    }
    keys
}

fn main() {
    let (total_ops, thread_count) = parse_cli_args();
    let ops_per_thread = total_ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;

    let dir = tempdir().expect("failed to create tempdir");
    let db_path = dir.path().join("bench_growth.db");
    let cfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(cfg).expect("failed to create turbofox instance");

    // Pre-allocate keys per worker in memory to isolate storage engine from PRNG
    let mut thread_keys = Vec::with_capacity(thread_count);
    for t in 0..thread_count {
        thread_keys.push(pregenerate_keys(t, ops_per_thread));
    }

    let payload = vec![0xAAu8; VAL_SIZE];
    let barrier = Arc::new(Barrier::new(thread_count + 1));
    let mut handles = Vec::with_capacity(thread_count);

    for keys in thread_keys {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = Histogram::<u64>::new_with_bounds(1, HISTOGRAM_MAX_NANOS, SIGFIG)
                .expect("failed to allocate histogram");

            barrier_clone.wait();

            for key in &keys {
                let t0 = Instant::now();
                let _ticket = db_clone.set(key, &payload_clone).expect("write failed");
                let elapsed_nanos = t0.elapsed().as_nanos() as u64;
                hist.record(elapsed_nanos).expect("record failed");
            }

            hist
        }));
    }

    // Coordinated launch across all worker threads
    barrier.wait();
    let bench_start = Instant::now();

    let mut combined_hist =
        Histogram::<u64>::new_with_bounds(1, HISTOGRAM_MAX_NANOS, SIGFIG).unwrap();

    for h in handles {
        let thread_hist = h.join().expect("worker thread panicked");
        combined_hist.add(&thread_hist).expect("failed to merge histograms");
    }

    let total_elapsed = bench_start.elapsed();
    let total_secs = total_elapsed.as_secs_f64();
    let ops_per_sec = (effective_total_ops as f64) / total_secs;

    let final_db_size = fs::read_dir(dir.path())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                .sum::<u64>()
        })
        .unwrap_or(0);
    let final_db_mib = (final_db_size as f64) / BYTES_PER_MIB;

    let p50 = (combined_hist.value_at_quantile(0.5) as f64) / NANOS_PER_MICRO;
    let p90 = (combined_hist.value_at_quantile(0.9) as f64) / NANOS_PER_MICRO;
    let p99 = (combined_hist.value_at_quantile(0.99) as f64) / NANOS_PER_MICRO;
    let p999 = (combined_hist.value_at_quantile(0.999) as f64) / NANOS_PER_MICRO;
    let max = (combined_hist.max() as f64) / NANOS_PER_MICRO;

    println!("==================================================");
    println!(" TurboFox Ingestion Benchmark (Growth Path)");
    println!("==================================================");
    println!(" Operations  : {:>8}", effective_total_ops);
    println!(" Workers     : {:>8}", thread_count);
    println!(" Wall time   : {:>8.3} s", total_secs);
    println!(" Throughput  : {:>8.0} ops/sec", ops_per_sec);
    println!(" DB Size     : {:>8.2} MiB", final_db_mib);
    println!("--------------------------------------------------");
    println!(" Latency (set call without sync wait):");
    println!("   p50       : {:>8.2} µs", p50);
    println!("   p90       : {:>8.2} µs", p90);
    println!("   p99       : {:>8.2} µs", p99);
    println!("   p999      : {:>8.2} µs", p999);
    println!("   max       : {:>8.2} µs", max);
    println!("==================================================");
}
