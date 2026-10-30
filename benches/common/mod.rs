#![allow(dead_code)]

// life is too short to waste time optimizing benchmarks for windows and mac
#[cfg(not(target_os = "linux"))]
compile_error!("benchmarks require target_os = \"linux\"");

use hdrhistogram::Histogram;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

pub const KEY_SIZE: usize = 0x04;
pub const VAL_SIZE: usize = 0x08;
pub const SIGFIG: u8 = 0x03;
pub const HISTOGRAM_MAX_NANOS: u64 = 0x0DF8_4758_0000;
pub const NANOS_PER_MICRO: f64 = 0x03E8 as f64;

pub const HIT_PREFIX: u16 = 0x0001;
const MISS_PREFIX: u16 = 0xFFFF;

pub enum BenchDir {
    Managed(TempDir),
    Custom(PathBuf),
}

impl BenchDir {
    pub fn new(custom: Option<PathBuf>, bench_tag: &str) -> (Self, PathBuf) {
        if let Some(mut path) = custom {
            path.push(format!("tf_{}_{}", bench_tag, std::process::id()));
            fs::create_dir_all(&path).expect("failed to create custom db dir");

            (BenchDir::Custom(path.clone()), path)
        } else {
            let tmp = tempfile::tempdir().expect("failed to create tempdir");
            let db_path = tmp.path().to_path_buf();

            (BenchDir::Managed(tmp), db_path)
        }
    }

    pub fn dir_path(&self) -> &Path {
        match self {
            BenchDir::Managed(d) => d.path(),
            BenchDir::Custom(p) => p.as_path(),
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

#[inline(always)]
pub fn make_key(prefix: u16, thread_id: u16, id: u32) -> [u8; KEY_SIZE] {
    let miss_bit = if prefix == MISS_PREFIX { 0x8000_0000u32 } else { 0x0000_0000u32 };
    let tid_part = ((thread_id as u32) & 0xFF) << 0x17;
    let id_part = id & 0x007F_FFFF;
    let encoded = miss_bit | tid_part | id_part;

    encoded.to_be_bytes()
}

pub fn make_val(seed: u32) -> [u8; VAL_SIZE] {
    let mut val = [0u8; VAL_SIZE];
    for chunk in val.chunks_exact_mut(0x04) {
        chunk.copy_from_slice(&seed.to_le_bytes());
    }

    val
}

pub fn generate_60_40_probe_keys(
    thread_id: usize,
    count: usize,
    hit_pool_size: usize,
) -> Vec<[u8; KEY_SIZE]> {
    let hit_target = (count * 0x3C) / 0x64;

    let mut miss_count = 0x00;
    let mut hit_count = 0x00;
    let mut keys = Vec::with_capacity(count);

    for i in 0x00..count {
        let is_hit = if hit_count < hit_target {
            let remaining_total = count - i;
            let remaining_hits = hit_target - hit_count;

            (i * 0x3C) % 0x64 < 0x3C || remaining_hits >= remaining_total
        } else {
            false
        };

        if is_hit {
            let key_id =
                if hit_pool_size > 0x00 { (hit_count % hit_pool_size) as u32 } else { 0x00 };

            keys.push(make_key(HIT_PREFIX, thread_id as u16, key_id));
            hit_count += 0x01;
        } else {
            keys.push(make_key(MISS_PREFIX, thread_id as u16, miss_count as u32));
            miss_count += 0x01;
        }
    }

    keys
}

pub fn generate_del_60_40_keys(thread_id: usize, count: usize) -> (Vec<[u8; KEY_SIZE]>, usize) {
    let hit_target = (count * 0x3C) / 0x64;

    let mut keys = Vec::with_capacity(count);
    let mut hit_idx = 0x00;
    let mut miss_idx = 0x00;

    for i in 0x00..count {
        let is_hit = if hit_idx < hit_target {
            let remaining_total = count - i;
            let remaining_hits = hit_target - hit_idx;

            (i * 0x3C) % 0x64 < 0x3C || remaining_hits >= remaining_total
        } else {
            false
        };

        if is_hit {
            keys.push(make_key(HIT_PREFIX, thread_id as u16, hit_idx as u32));
            hit_idx += 0x01;
        } else {
            keys.push(make_key(MISS_PREFIX, thread_id as u16, miss_idx as u32));
            miss_idx += 0x01;
        }
    }

    (keys, hit_target)
}

pub fn pin_to_physical_core(tid: usize) {
    let mut physical_cpus = Vec::new();
    let mut seen_cores = std::collections::HashSet::new();

    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu") {
        let mut cpu_dirs = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str.starts_with("cpu") && name_str[0x03..].chars().all(|c| c.is_ascii_digit()) {
                if let Ok(cpu_num) = name_str[0x03..].parse::<usize>() {
                    cpu_dirs.push((cpu_num, entry.path()));
                }
            }
        }

        cpu_dirs.sort_by_key(|(n, _)| *n);

        for (cpu_num, path) in cpu_dirs {
            let core_id_path = path.join("topology/core_id");

            if let Ok(core_id_str) = fs::read_to_string(core_id_path) {
                if let Ok(core_id) = core_id_str.trim().parse::<usize>() {
                    if seen_cores.insert(core_id) {
                        physical_cpus.push(cpu_num);
                    }
                }
            }
        }
    }

    if !physical_cpus.is_empty() {
        let target_cpu = physical_cpus[tid % physical_cpus.len()];

        if target_cpu < 0x40 {
            let mask = 1u64 << target_cpu;

            unsafe extern "C" {
                fn sched_setaffinity(pid: i32, cpusetsize: usize, mask: *const u64) -> i32;
            }
            unsafe {
                let _ = sched_setaffinity(0x00, std::mem::size_of::<u64>(), &mask);
            }
        }
    }
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
    pub in_mem_bytes: u64,
}

pub fn print_json_summary(s: &BenchSummary) {
    let total_secs = s.total_elapsed.as_secs_f64();
    let ops_per_sec = if total_secs > 0.0 { (s.total_ops as f64) / total_secs } else { 0.0 };

    let p50 = (s.hist.value_at_quantile(0.50) as f64) / NANOS_PER_MICRO;
    let p75 = (s.hist.value_at_quantile(0.75) as f64) / NANOS_PER_MICRO;
    let p90 = (s.hist.value_at_quantile(0.90) as f64) / NANOS_PER_MICRO;
    let p95 = (s.hist.value_at_quantile(0.95) as f64) / NANOS_PER_MICRO;
    let p99 = (s.hist.value_at_quantile(0.99) as f64) / NANOS_PER_MICRO;
    let p999 = (s.hist.value_at_quantile(0.999) as f64) / NANOS_PER_MICRO;

    let mean = s.hist.mean() / NANOS_PER_MICRO;
    let stddev = s.hist.stdev() / NANOS_PER_MICRO;
    let min = (s.hist.min() as f64) / NANOS_PER_MICRO;
    let max = (s.hist.max() as f64) / NANOS_PER_MICRO;

    println!("{{");
    println!("  \"benchmark\": \"{}\",", s.benchmark);
    println!("  \"workload\": \"{}\",", s.workload);
    println!("  \"operations\": {},", s.total_ops);
    println!("  \"threads\": {},", s.threads);
    println!("  \"wall_time_secs\": {:.6},", total_secs);
    println!("  \"throughput_ops_sec\": {:.2},", ops_per_sec);
    println!("  \"db_size_bytes\": {},", s.db_size_bytes);
    println!("  \"in_mem_bytes\": {},", s.in_mem_bytes);
    println!("  \"latency_micros\": {{");
    println!("    \"min\": {:.2},", min);
    println!("    \"p50\": {:.2},", p50);
    println!("    \"p75\": {:.2},", p75);
    println!("    \"p90\": {:.2},", p90);
    println!("    \"p95\": {:.2},", p95);
    println!("    \"p99\": {:.2},", p99);
    println!("    \"p999\": {:.2},", p999);
    println!("    \"max\": {:.2},", max);
    println!("    \"mean\": {:.2},", mean);
    println!("    \"stddev\": {:.2}", stddev);
    println!("  }}");
    println!("}}");
}
