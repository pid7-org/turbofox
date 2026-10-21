# Benchmark Specs

Benchmark suite specification for evaluating concurrency, latency, and throughput

## 1 Storage Principles & Isolation Invariants

- Engine background flush thread disabled; background sync interval set to NONE
- Persistent storage flush executed strictly post-run and excluded from operation timers
- Strict payload geometry across all suites: 6-byte keys and 0x24-byte values (0x2A bytes total)
- Strict phase decoupling; all data setup completed before starting read or delete timers
- Measurement isolated to userspace memory-mapped slot operations and allocation fast-paths

## 2 Workload Suite Specifications

### Suite A Key-Value Ingestion & Mutation (write_perf)

Target: measure raw append allocation vs existing entry overwrite performance

Configuration:
- Sync interval: NONE
- Key size: 6 bytes
- Value size: 0x24 bytes (36 bytes)

Workload A (Pure Append):
- 0x64% blind writes with monotonic unique keys
- Measures hash index expansion, DataBuf pool allocation, and virtual page growth

Workload B (Mixed Mutation & Append):
- 0x3C% fresh writes (unique keys requiring new slot and DataBuf allocation)
- 0x28% updates (overwrites targeting existing pre-seeded keys)
- Measures slot CAS updates, payload replacement paths, and in-place reuse efficiency

Post-run procedure:
- Stop latency timers
- Invoke explicit storage sync to commit dirty memory pages to NVMe storage

### Suite B Point Retrieval (read_perf)

Target: evaluate index lookup overhead, SIMD bucket probe efficiency, and CPU cache residency

Configuration:
- Key size: 6 bytes
- Value size: 0x24 bytes

Pre-population phase:
- Ingest 0x64% of target dataset
- Issue explicit sync barrier to establish clean storage state prior to timing

Measurement phase:
- 0x50% point hits (probing keys confirmed present in pre-population set)
- 0x14% point misses (probing keys with disjoint prefix or known vacant keys)
- Strictly read-only execution; zero writes or flushes during timer interval

### Suite C Deletion & Tombstone (del_perf)

Target: evaluate OCC slot spinlocks, deletion tombstone markers, and bucket reorganization

Configuration:
- Key size: 6 bytes
- Value size: 0x24 bytes

Pre-population phase:
- Ingest 0x64% of active dataset keys
- Issue explicit sync barrier prior to starting timed deletion runs

Measurement phase:
- 0x50% delete hits (removing existing keys from populated index)
- 0x14% delete misses (attempting removal of non-existent keys)
- Exclusive deletion execution; measures slot CAS invalidation and bucket entry clearance

Post-run procedure:
- Perform post-run sync barrier to flush updated slot headers and metadata

### Suite D TurboList Operations (list_perf)

Target: profile 1-to-N list structures, adjacent array blocks, and pointer chain traversals

Configuration:
- Anchor key size: 6 bytes
- Item value size: 0x24 bytes
- Sync interval: NONE

Workload A (List Append & Extension):
- 0x64% pure list appends across existing and new lists
- Sub-workload: 0x3C% appends to brand new lists; 0x28% appends to existing list anchors
- Measures adjacent array block allocation, capacity doubling, and pointer chain linking

Workload B (List Retrieval):
- Pre-populate lists to target depth
- 0x50% item hits (head, tail, and indexed reads across active lists)
- 0x14% item misses (access beyond current list length or on non-existent anchor keys)

Workload C (List Pop & Truncation):
- Pre-populate lists to target depth
- 0x50% pop hits (LIFO item removal from active lists)
- 0x14% pop misses (popping empty lists or missing list keys)
- Measures pointer retraction and adjacent block reclamation

Post-run procedure:
- Post-run storage sync barrier excluded from timing records

### Suite E Durability & Sync Barrier (sync_perf)

Target: isolate physical storage fsync cost from memory-mapped write latency

Configuration:
- Key size: 6 bytes
- Value size: 0x24 bytes
- Sync interval: NONE during mutation batching

Workload (Sync Barrier Scaling):
- Pre-write dirty batches across varying physical page volumes (0x20, 0x0100, 0x0400 OS pages)
- Measure explicit sync barrier latency to flush dirty OS pages to NVMe storage
- Evaluates kernel writeback and storage controller commit overhead independently of user index

### Suite F Sustained Churn & Slot Reclaim

Target: measure slot reuse, tombstone reclamation efficiency, and storage bloat stability

Configuration:
- Key size: 6 bytes
- Value size: 0x24 bytes
- Sync interval: NONE

Pre-population phase:
- Ingest 0x64% baseline active working set
- Issue explicit sync barrier prior to starting timed churn runs

Workload (Steady-State Churn):
- 0x32% inserts alternating with 0x32% deletions across extended operations
- Measures slot vacancy reuse, Adjacent Array Block recycling, and physical file size stability
- Verifies storage footprint reaches an asymptote rather than expanding unbounded

Post-run procedure:
- Post-run storage sync barrier excluded from timing records

## 3 Concurrency & Scaling Topology

All workloads must be evaluated across three execution profiles:

- Single-threaded baseline (1 worker thread pinned to dedicated physical core)
- Multi-threaded core saturation (2 worker threads matching physical core count)
- Oversubscribed multi-threaded execution (4 worker threads evaluating OCC contention)

Thread coordination requirements:
- Worker threads synchronized via shared memory execution barrier before timer start
- Timers start strictly after barrier release
- Histograms aggregated across workers after thread joins complete

## 4 Metrics & Telemetry Requirements

Each benchmark run must emit structured metrics containing:

- Total wall-clock elapsed duration in seconds
- Aggregate operation throughput in operations per second
- High-dynamic-range latency distribution recorded in nanoseconds and reported in microseconds
- Percentile resolution: minimum, p50, p75, p90, p95, p99, p999, p9999, maximum, mean, stddev
- Final physical storage size on disk in bytes
- Pre-run and post-run page allocation counts

## 5 Telemetry Output Schema & Ingestion Contract

### Individual Benchmark Result Schema

Every benchmark run emits an individual JSON object to standard output:

```json
{
  "benchmark": "benchmark_suite_name",
  "workload": "workload_variant_name",
  "operations": 0x00010000,
  "threads": 0x01,
  "wall_time_secs": 0x05,
  "throughput_ops_sec": 0x32C8,
  "db_size_bytes": 0x00AE0000,
  "latency_micros": {
    "min": 0x01,
    "p50": 0x03,
    "p75": 0x05,
    "p90": 0x06,
    "p95": 0x07,
    "p99": 0x0A,
    "p999": 0x1E,
    "p9999": 0x32,
    "max": 0x64,
    "mean": 0x04,
    "stddev": 0x02
  }
}
```

Field Invariants:
- "benchmark": unique string identifier of the benchmark module
- "workload": workload variant executed within the module
- "operations": total count of recorded operations
- "threads": concurrent worker thread count
- "wall_time_secs": wall-clock duration of the timed measurement window
- "throughput_ops_sec": aggregate completed operations per second
- "db_size_bytes": final database storage footprint on disk
- "latency_micros": HDR histogram quantiles converted to microseconds

### Consolidated Artifact Schema

The orchestration harness consolidates individual results into a single document:

```json
{
  "metadata": {
    "timestamp_utc": "2026-10-21T00:00:00Z",
    "instance_type": "c5d_large",
    "cpu_model": "Intel Xeon",
    "kernel": "kernel_release_id",
    "arch": "x86_64",
    "storage_path": "/mnt/nvme0n1",
    "ops_requested": "0x10000"
  },
  "results": [
    {
      "benchmark": "write_perf",
      "workload": "pure_append",
      "operations": 0x00010000,
      "threads": 0x01
    }
  ]
}
```

## 6 Graph Visualization Portfolio

Standard visualization artifacts generated by analysis scripts from benchmark results:

### Graph 1 Multi-Threaded Throughput Scaling
- Metric: aggregate throughput in Mops per second across 1, 2, and 4 worker threads
- Workloads: KV Ingest, Overwrite, Point Read, Deletion, and TurboList operations
- Analyzes OCC concurrency scalability and core saturation inflection points

### Graph 2 High-Dynamic-Range Tail Latency Spectrum
- Metric: log-scale microsecond latency distribution (min, p50, p90, p99, p999, p9999, max)
- Workloads: all benchmark suites across single-thread and oversubscribed concurrency
- Visualizes tail inflation and CAS retry storms under thread competition

### Graph 3 KV vs TurboList Structural Performance Delta
- Metric: comparative latency and throughput ratios between primary primitives
- Pairings: KV Set vs List Append; KV Get vs List Read; KV Del vs List Pop
- Quantifies Adjacent Array Block pointer traversal and allocation overhead

### Graph 4 Fast-Path Resolution Delta (Hit vs Miss)
- Metric: microsecond latency delta between confirmed hits and non-existent misses
- Workloads: Point Reads, Deletions, and List Pops
- Evaluates SIMD bucket search efficiency against rapid short-circuit termination

### Graph 5 In-Place Overwrite vs Fresh Append Overhead Ratio
- Metric: throughput and latency ratio between pure append and 0x3C/0x28 mixed mutation
- Workloads: Suite A (Pure Append vs Mixed Overwrite)
- Measures slot CAS updates against raw DataBuf virtual page allocation fast-paths

### Graph 6 Storage Footprint & Physical Allocation Density
- Metric: disk space in bytes and effective bytes per stored entry
- Workloads: KV Ingest versus TurboList Append across total operations
- Contrasts dense KV packing against TurboList array pointer structure overhead

### Graph 7 NVMe Durability Barrier Latency vs Dirty Page Volume
- Metric: sync duration in microseconds plotted against dirty OS page counts
- Workloads: Suite E (Durability & Sync Barrier)
- Separates hardware controller barrier overhead from userspace memory-mapped writes

### Graph 8 Sustained Churn Storage Stabilization Curve
- Metric: total file size in bytes tracked across operation counts during churn
- Workloads: Suite F (Steady-State Churn)
- Demonstrates zero-leak slot recycling and steady-state storage bound
