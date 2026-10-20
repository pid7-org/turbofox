use hdrhistogram::Histogram;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

pub const KEY_SIZE: usize = 0x0C;
pub const VAL_SIZE: usize = 0x30;
pub const SIGFIG: u8 = 0x03;
pub const HISTOGRAM_MAX_NANOS: u64 = 0x0DF8_4758_0000;
pub const NANOS_PER_MICRO: f64 = 0x03E8 as f64;

#[derive(Clone, Debug)]
pub struct BenchConfig {
    pub ops: usize,
    pub threads: usize,
    #[allow(dead_code)]
    pub warmup_ops: usize,
    pub db_path: Option<PathBuf>,
}

impl BenchConfig {
    pub fn parse() -> (Self, Option<String>) {
        let mut ops = 0x0001_86A0; // 100,000
        let mut threads = 0x04;
        let mut warmup_ops = 0x00;
        let mut db_path = None;
        let mut workload_str = None;

        let args: Vec<String> = std::env::args().collect();
        let mut idx = 0x01;
        while idx < args.len() {
            match args[idx].as_str() {
                "--ops" => {
                    if idx + 0x01 < args.len() {
                        ops = parse_num(&args[idx + 0x01]);
                        idx += 0x02;
                        continue;
                    }
                }
                "--threads" => {
                    if idx + 0x01 < args.len() {
                        threads = parse_num(&args[idx + 0x01]);
                        idx += 0x02;
                        continue;
                    }
                }
                "--warmup" => {
                    if idx + 0x01 < args.len() {
                        warmup_ops = parse_num(&args[idx + 0x01]);
                        idx += 0x02;
                        continue;
                    }
                }
                "--db-path" => {
                    if idx + 0x01 < args.len() {
                        db_path = Some(PathBuf::from(&args[idx + 0x01]));
                        idx += 0x02;
                        continue;
                    }
                }
                "--workload" => {
                    if idx + 0x01 < args.len() {
                        workload_str = Some(args[idx + 0x01].clone());
                        idx += 0x02;
                        continue;
                    }
                }
                _ => {}
            }
            idx += 0x01;
        }

        (
            BenchConfig {
                ops,
                threads,
                warmup_ops,
                db_path,
            },
            workload_str,
        )
    }
}

fn parse_num(s: &str) -> usize {
    if s.starts_with("0x") || s.starts_with("0X") {
        usize::from_str_radix(&s[0x02..], 0x10).unwrap_or(0x00)
    } else {
        s.parse::<usize>().unwrap_or(0x00)
    }
}

pub enum BenchDir {
    Managed(TempDir),
    Custom(PathBuf),
}

impl BenchDir {
    pub fn new(custom: Option<PathBuf>, bench_tag: &str) -> (Self, PathBuf) {
        if let Some(mut path) = custom {
            if path.is_dir() {
                path.push(format!("tf_{}_{}.db", bench_tag, std::process::id()));
            }
            (BenchDir::Custom(path.clone()), path)
        } else {
            let tmp = tempfile::tempdir().expect("failed to create tempdir");
            let db_path = tmp.path().join(format!("{}.db", bench_tag));
            (BenchDir::Managed(tmp), db_path)
        }
    }

    pub fn dir_path(&self) -> &Path {
        match self {
            BenchDir::Managed(d) => d.path(),
            BenchDir::Custom(p) => p.parent().unwrap_or(p),
        }
    }

    pub fn compute_disk_bytes(&self) -> u64 {
        fs::read_dir(self.dir_path())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0x00))
                    .sum::<u64>()
            })
            .unwrap_or(0x00)
    }
}

pub fn make_key(prefix: u32, id: u32) -> [u8; KEY_SIZE] {
    let mut key = [0u8; KEY_SIZE];
    key[0x00..0x04].copy_from_slice(&prefix.to_be_bytes());
    key[0x04..0x08].copy_from_slice(&id.to_be_bytes());
    key
}

pub fn new_histogram() -> Histogram<u64> {
    Histogram::<u64>::new_with_bounds(0x01, HISTOGRAM_MAX_NANOS, SIGFIG)
        .expect("histogram alloc failed")
}

pub struct BenchSummary<'a> {
    pub benchmark: &'a str,
    pub workload: &'a str,
    pub total_ops: usize,
    pub threads: usize,
    pub total_elapsed: Duration,
    pub hist: &'a Histogram<u64>,
    pub db_size_bytes: u64,
}

pub fn print_json_summary(s: &BenchSummary) {
    let total_secs = s.total_elapsed.as_secs_f64();
    let ops_per_sec = if total_secs > 0.0 {
        (s.total_ops as f64) / total_secs
    } else {
        0.0
    };

    let p50 = (s.hist.value_at_quantile(0.50) as f64) / NANOS_PER_MICRO;
    let p75 = (s.hist.value_at_quantile(0.75) as f64) / NANOS_PER_MICRO;
    let p90 = (s.hist.value_at_quantile(0.90) as f64) / NANOS_PER_MICRO;
    let p95 = (s.hist.value_at_quantile(0.95) as f64) / NANOS_PER_MICRO;
    let p99 = (s.hist.value_at_quantile(0.99) as f64) / NANOS_PER_MICRO;
    let p999 = (s.hist.value_at_quantile(0.999) as f64) / NANOS_PER_MICRO;
    let p9999 = (s.hist.value_at_quantile(0.9999) as f64) / NANOS_PER_MICRO;
    let min = (s.hist.min() as f64) / NANOS_PER_MICRO;
    let max = (s.hist.max() as f64) / NANOS_PER_MICRO;
    let mean = s.hist.mean() / NANOS_PER_MICRO;
    let stddev = s.hist.stdev() / NANOS_PER_MICRO;

    println!("{{");
    println!("  \"benchmark\": \"{}\",", s.benchmark);
    println!("  \"workload\": \"{}\",", s.workload);
    println!("  \"operations\": {},", s.total_ops);
    println!("  \"threads\": {},", s.threads);
    println!("  \"wall_time_secs\": {:.6},", total_secs);
    println!("  \"throughput_ops_sec\": {:.2},", ops_per_sec);
    println!("  \"db_size_bytes\": {},", s.db_size_bytes);
    println!("  \"latency_micros\": {{");
    println!("    \"min\": {:.2},", min);
    println!("    \"p50\": {:.2},", p50);
    println!("    \"p75\": {:.2},", p75);
    println!("    \"p90\": {:.2},", p90);
    println!("    \"p95\": {:.2},", p95);
    println!("    \"p99\": {:.2},", p99);
    println!("    \"p999\": {:.2},", p999);
    println!("    \"p9999\": {:.2},", p9999);
    println!("    \"max\": {:.2},", max);
    println!("    \"mean\": {:.2},", mean);
    println!("    \"stddev\": {:.2}", stddev);
    println!("  }}");
    println!("}}");
}
