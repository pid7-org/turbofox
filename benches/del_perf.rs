use hdrhistogram::Histogram;
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
const HISTOGRAM_MAX_NANOS: u64 = 0x0DF8_4758_0000;
const CONTENTION_POOL: usize = 0x40;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Workload {
    Hit,
    Miss,
    Churn,
    Contention,
}

impl Workload {
    fn from_str(s: &str) -> Self {
        match s {
            "miss" => Self::Miss,
            "churn" => Self::Churn,
            "contention" => Self::Contention,
            _ => Self::Hit,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "100% Delete Hit (Disjoint)",
            Self::Miss => "100% Delete Miss (Negative Lookup)",
            Self::Churn => "50% Insert / 50% Delete (Steady State)",
            Self::Contention => "High CAS Contention (Shared Keyspace)",
        }
    }
}

fn parse_cli_args() -> (usize, usize, Workload) {
    let mut ops = DEFAULT_OPS;
    let mut threads = DEFAULT_THREADS;
    let mut workload = Workload::Hit;
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
        } else if args[idx] == "--workload" && idx + 1 < args.len() {
            workload = Workload::from_str(&args[idx + 1]);
            idx += 2;
        } else {
            idx += 1;
        }
    }
    (ops, threads, workload)
}

fn make_key(prefix: u32, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0..0x04].copy_from_slice(&prefix.to_be_bytes());
    key[0x04..0x08].copy_from_slice(&id.to_be_bytes());
    key
}

fn pregenerate_del_keys(
    workload: Workload,
    thread_id: usize,
    count: usize,
    _dataset_size: usize,
) -> Vec<[u8; KEY_SIZE]> {
    let mut keys = Vec::with_capacity(count);
    for i in 0..count {
        match workload {
            Workload::Hit => {
                let id = thread_id * count + i;
                keys.push(make_key(1, id as u32));
            }
            Workload::Miss => {
                let id = thread_id * count + i;
                keys.push(make_key(0xFF, id as u32));
            }
            Workload::Churn => {
                let id = thread_id * count + i;
                keys.push(make_key(1, id as u32));
            }
            Workload::Contention => {
                let id = (i * DEFAULT_THREADS + thread_id) % CONTENTION_POOL;
                keys.push(make_key(1, id as u32));
            }
        }
    }
    keys
}

fn main() {
    let (total_ops, thread_count, workload) = parse_cli_args();
    let ops_per_thread = total_ops / thread_count;
    let effective_total_ops = ops_per_thread * thread_count;
    let dataset_size =
        if workload == Workload::Contention { CONTENTION_POOL } else { effective_total_ops };

    let dir = tempdir().expect("failed to create tempdir");
    let db_path = dir.path().join("bench_del.db");
    let cfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(cfg).expect("failed to create turbofox instance");

    let payload = vec![0x5Au8; VAL_SIZE];

    // Pre-populate database with seed keys
    for i in 0..dataset_size {
        let key = make_key(1, i as u32);
        let _ = db.set(&key, &payload).expect("seeding set failed");
    }
    db.sync().expect("pre-population sync failed");

    // Pre-allocate keys per worker in memory to isolate storage engine from PRNG
    let mut thread_keys = Vec::with_capacity(thread_count);
    for t in 0..thread_count {
        thread_keys.push(pregenerate_del_keys(workload, t, ops_per_thread, dataset_size));
    }

    let barrier = Arc::new(Barrier::new(thread_count + 1));
    let mut handles = Vec::with_capacity(thread_count);

    for (t, keys) in thread_keys.into_iter().enumerate() {
        let db_clone = db.clone();
        let barrier_clone = barrier.clone();
        let payload_clone = payload.clone();

        handles.push(thread::spawn(move || {
            let mut hist = Histogram::<u64>::new_with_bounds(1, HISTOGRAM_MAX_NANOS, SIGFIG)
                .expect("failed to allocate histogram");

            barrier_clone.wait();

            for (idx, key) in keys.iter().enumerate() {
                if workload == Workload::Churn {
                    let new_id = (t * ops_per_thread + idx) as u32;
                    let churn_key = make_key(2, new_id);
                    let _ = db_clone.set(&churn_key, &payload_clone).expect("churn set failed");
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("churn del failed");
                    let elapsed = t0.elapsed().as_nanos() as u64;
                    hist.record(elapsed).expect("record failed");
                } else {
                    let t0 = Instant::now();
                    let _ = db_clone.del(key).expect("del failed");
                    let elapsed = t0.elapsed().as_nanos() as u64;
                    hist.record(elapsed).expect("record failed");
                }
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

    let p50 = (combined_hist.value_at_quantile(0.5) as f64) / NANOS_PER_MICRO;
    let p90 = (combined_hist.value_at_quantile(0.9) as f64) / NANOS_PER_MICRO;
    let p99 = (combined_hist.value_at_quantile(0.99) as f64) / NANOS_PER_MICRO;
    let p999 = (combined_hist.value_at_quantile(0.999) as f64) / NANOS_PER_MICRO;
    let max = (combined_hist.max() as f64) / NANOS_PER_MICRO;

    println!("==================================================");
    println!(" TurboFox Delete Performance Benchmark");
    println!("==================================================");
    println!(" Workload    : {}", workload.as_str());
    println!(" Operations  : {:>8}", effective_total_ops);
    println!(" Workers     : {:>8}", thread_count);
    println!(" Wall time   : {:>8.3} s", total_secs);
    println!(" Throughput  : {:>8.0} ops/sec", ops_per_sec);
    println!("--------------------------------------------------");
    println!(" Latency (del call):");
    println!("   p50       : {:>8.2} µs", p50);
    println!("   p90       : {:>8.2} µs", p90);
    println!("   p99       : {:>8.2} µs", p99);
    println!("   p999      : {:>8.2} µs", p999);
    println!("   max       : {:>8.2} µs", max);
    println!("==================================================");
}
