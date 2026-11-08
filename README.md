<img src="assets/banner.png" width="100%" alt="TurboFox" />

[![Crates.io](https://img.shields.io/crates/v/turbofox?style=flat-square&logo=rust&logoColor=eff1f5&color=1e66f5&labelColor=4c4f69)](https://crates.io/crates/turbofox)
[![Docs.rs](https://img.shields.io/docsrs/turbofox?style=flat-square&logo=docs.rs&logoColor=eff1f5&color=179299&labelColor=4c4f69)](https://docs.rs/turbofox)
[![Tests](https://img.shields.io/github/actions/workflow/status/pid7-org/turbofox/tests.yaml?style=flat-square&logo=github&logoColor=eff1f5&label=tests&color=40a02b&labelColor=4c4f69)](https://github.com/pid7-org/turbofox/actions/workflows/tests.yaml)
[![MSRV](https://img.shields.io/badge/MSRV-1.89.0-fe640b?style=flat-square&logo=rust&logoColor=eff1f5&labelColor=4c4f69)](#usage)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-8839ef?style=flat-square&logo=open-source-initiative&logoColor=eff1f5&labelColor=4c4f69)](LICENSE-MIT)

TurboFox is an embedded, memory-mapped key-value engine providing lock-free concurrency & explicit
sync barriers

> [!IMPORTANT]
> _TurboFox_ is under active development
>
> Architecture and disk layout remain subject to breaking changes, hence production use is
> discouraged but not prohibited, given _you assume all the risks_

## Index

- [Usage](#usage)
- [Features](#features)
  - [Key-Value Operations](#key-value-operations)
  - [TurboList](#turbolist)
  - [Explicit Durability & Sync Barriers](#explicit-durability--sync-barriers)
  - [Page Eviction & Resource Telemetry](#page-eviction--resource-telemetry)
- [Lifecycle & Initialization](#lifecycle--initialization)
- [Architecture Guarantees](#architecture-guarantees)
  - [Zero Global Rehash](#zero-rehash)
  - [Torn-Write Protection](#torn-write-protection)
  - [Durability Model](#durability-model)
  - [RAII Safety](#raii-safety)
  - [Graceful Shutdown](#graceful-shutdown)
  - [Storage Topography](#storage-topography)
  - [Operational Invariants](#operational-invariants)
- [Supported Platforms](#supported-platforms)
- [Thread Safety](#thread-safety)
- [Benchmarks](#benchmarks)
  - [§1 · Concurrency Dynamics & Throughput Scaling](#1--concurrency-dynamics--throughput-scaling)
  - [§2 · Tail Latency Spectrum & Percentile CDF](#2--tail-latency-spectrum--percentile-cdf)
  - [§3 · Memory Residency & Resident Set Size (RSS)](#3--memory-residency--resident-set-size-rss)
  - [§4 · Cold-Open Latency & Index Memory Density](#4--cold-open-latency--index-memory-density)

## Usage

Add `turbofox` to your `Cargo.toml`:

```toml
[dependencies]
turbofox = "0.0.300"
```

> [!NOTE]
> Minimum Supported Rust Version (MSRV) is `1.89.0`

### Quickstart

```rust
use turbofox::{TurboCfg, TurboFox};

let dir = tempfile::tempdir().unwrap();
let path = dir.path();

let cfg = TurboCfg::manual(path);
let db = TurboFox::open_or_create(cfg).unwrap();

let ticket = db.set(b"device:01", b"state_active").unwrap();
assert_eq!(db.get(b"device:01").unwrap(), Some(b"state_active".to_vec()));

db.sync().unwrap();
assert!(ticket.is_durable());

assert!(db.del(b"device:01").unwrap());
assert_eq!(db.get(b"device:01").unwrap(), None);

let list = db.list(b"events");

list.append(b"boot").unwrap().sync().unwrap();
list.append(b"init").unwrap().sync().unwrap();

assert_eq!(list.len().unwrap(), 2);
assert_eq!(list.pop().unwrap(), Some(b"init".to_vec()));

drop(list);
db.delete().unwrap();
```

## Features

### Key-Value Operations

Lock-free point reads, writes, and deletions backed by memory-mapped storage and optimistic
concurrency control:

* Writes commit immediately into memory-mapped pages and return a `TurboTicket` bound to a
  monotonic commit epoch
* Reads validate CRC32 checksum integrity and byte-level key equality; torn reads during concurrent
  writes fail safely via `TurboError::CorruptedPayload`
* Probing via `contains_key` checks index presence without allocating or copying value payloads to
  the heap
* In-place overwrites execute directly when updated values fit in existing allocated space
* Deletions via `del` reclaim key-value slots in place and return existence status

```rust
use turbofox::{TurboCfg, TurboFox};

let dir = tempfile::tempdir().unwrap();
let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();

let ticket = db.set(b"session:0x42", b"user_active").unwrap();
assert!(ticket.epoch() > 0);

assert!(db.contains_key(b"session:0x42").unwrap());

let val = db.get(b"session:0x42").unwrap();
assert_eq!(val, Some(b"user_active".to_vec()));

db.set(b"session:0x42", b"user_idle").unwrap();
assert_eq!(db.get(b"session:0x42").unwrap(), Some(b"user_idle".to_vec()));

assert!(db.del(b"session:0x42").unwrap());
assert!(!db.contains_key(b"session:0x42").unwrap());

db.delete().unwrap();
```

### TurboList

Ordered 1-to-N sequences co-exist inside the primary storage file without secondary index tables,
auxiliary catalog files, or relational join overhead:

* Sequential appends and pops execute with amortized O(1) complexity
* Boundary inspection via `first` and `last` inspects queue extremes without traversing the list
* Fast indexed random access via `get_at` fetches items by zero-based ordinal position
* Forward snapshot iteration via `iter` traverses active entries sequentially
* Per-item CRC32 checksums validate item integrity on every read

```rust
use turbofox::{TurboCfg, TurboFox};

let dir = tempfile::tempdir().unwrap();
let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();

let list = db.list(b"jobs:pending");

list.append(b"task_1").unwrap();
list.append(b"task_2").unwrap();
list.append(b"task_3").unwrap();
assert_eq!(list.len().unwrap(), 3);

assert_eq!(list.first().unwrap(), Some(b"task_1".to_vec()));
assert_eq!(list.last().unwrap(), Some(b"task_3".to_vec()));
assert_eq!(list.get_at(1).unwrap(), Some(b"task_2".to_vec()));

let mut iter = list.iter().unwrap();
assert_eq!(iter.next(), Some(b"task_1".to_vec()));
assert_eq!(iter.next(), Some(b"task_2".to_vec()));
assert_eq!(iter.next(), Some(b"task_3".to_vec()));
assert_eq!(iter.next(), None);
drop(iter);

assert_eq!(list.pop().unwrap(), Some(b"task_3".to_vec()));
assert_eq!(list.len().unwrap(), 2);

list.clear().unwrap();
assert!(list.is_empty().unwrap());

drop(list);
db.delete().unwrap();
```

### Explicit Durability & Sync Barriers

Decouples memory-speed mutation throughput from physical NVMe writeback latency:

* Mutations (`set`, `del`, `append`, `pop`) return a lightweight `TurboTicket` bound to a
  monotonic commit epoch
* Non-blocking verification via `ticket.is_durable()` checks physical writeback status
  asynchronously
* Explicit sync barriers via `ticket.wait()`, `ticket.force()`, or `db.sync()` guarantee
  persistence to physical media
* Configurable sync strategies support unmanaged manual flushing (`SyncMode::Manual`) and
  adaptive background flushing (`SyncMode::Periodic`)

### Page Eviction & Resource Telemetry

Granular control over OS page cache residency and storage footprint:

* Advisory cache trimming via `evict` advises host kernel page tables to release clean file-backed
  pages (`PreserveHeaders` or `PurgeAll`) to relieve physical RAM pressure
* Telemetry queries inspect active page cache residency via `resident_bytes_in_memory()` and
  allocated file size via `resident_bytes_on_disk()`
* Virtual address space expands lazily up to 0x40_0000_0000 bytes (256 GiB) while physical file
  size starts at only 16 KiB

## Lifecycle & Initialization

TurboFox provides three initialization primitives,

* [`TurboFox::open_or_create`]: Recommended for general application lifecycles; checks
  backing storage presence and initializes if missing, while strictly propagating corruption
  or validation errors rather than silently clobbering existing state
* [`TurboFox::open`]: Strict open; requires existing database files and validates magic,
  layout version, page alignment, and header integrity; fails if storage is absent
* [`TurboFox::create`]: Strict creation; initializes physical pages and format metadata;
  fails with [`TurboError::Storage`] if storage files already exist

TurboFox provides three distinct initialization entry points to match operational requirements,

| Method           | Target Exists | Target Missing | Target Corrupt | Recommended Use Case                     |
|:-----------------|:------------- |:---------------|:---------------|:-----------------------------------------|
| `open_or_create` | Opens DB      | Inits DB       | Fails (`Err`)  | General application boot sequences       |
| `create`         | Fails (`Err`) | Inits DB       | N/A            | Cold setup or provisioning scripts       |
| `open`           | Opens DB      | Fails (`Err`)  | Fails (`Err`)  | Strict restarts requiring existing state |

## Architecture Guarantees

TurboFox is an embedded key-value storage engine engineered around memory-mapped persistence,
optimistic concurrency control, and zero-rehash index chaining

Reference documentation and method signatures are available on [docs.rs](https://docs.rs/turbofox)

### Zero Global Rehash

Primary key routing uses 32-bit XxHash32 with persistent index chaining:

* Key entries are permanently anchored without global table redistribution, eliminating
  stop-the-world rehashing pauses and maintaining bounded p99 write latency
* In-place overwrites execute directly when updated values fit in existing allocated space
* Storage expands incrementally via monotonic allocation without bulk page copying pauses

### Torn-Write Protection

Point reads execute concurrently without reader-writer locks or shared mutex acquisition:

* Reading automatically validates payload CRC32 integrity alongside byte-level key equality
* Concurrent writes or partial page flushes that cause torn reads fail safely via
  `TurboError::CorruptedPayload` rather than returning corrupt data
* Non-existent keys short-circuit quickly without accessing payload storage pages; zero heap
  allocations occur during lookup misses

### Durability Model

Mutations commit immediately via optimistic concurrency control and anchor to a monotonic commit
epoch:

* Writing memory pages and committing the in-memory index occurs atomically before releasing OCC
  transaction records
* Physical persistence tracks monotonically advancing flush epochs; readers observe committed
  data immediately while durability guarantees remain decoupled to NVMe writeback barriers
* Unwritten dirty pages survive process crashes if flushed by the OS page cache, but require
  explicit barriers (`db.sync()` or `ticket.wait()`) to guarantee crash safety against host power
  loss

### RAII Safety

> [!NOTE]
>
> `TurboFox` is RAII-safe with respect to system resource reclamation
>
> Dropping a database handle cleanly releases all allocated heap buffers, unmaps virtual memory
> regions, and closes backing OS file descriptors alongside exclusive advisory file locks without
> leaks

### Graceful Shutdown

> [!WARNING]
>
> RAII resource cleanup must not be conflated with durability guarantees
>
> Dropping an instance initiates a best-effort background flush, but Rust's `Drop` interface
> discards return values, hence delayed writeback failures (such as physical I/O errors or storage
> exhaustion) cannot be propagated to the caller and will fail silently
>
> Applications performing graceful shutdown must establish an explicit durability barrier before
> dropping the instance:
>
> * Under periodic sync configurations, await outstanding write completion via `ticket.wait()`
>   or poll `ticket.is_durable()`
> * Under manual sync configurations, trigger a synchronous write barrier via `ticket.force()`
>   or `db.sync()`, verifying the returned `Result`

### Storage Topography

Unified single-file storage container managed across memory-mapped virtual address space:

* Initial file footprint begins at only 16 KiB on standard host systems
* Virtual memory space reserves up to `MAX_DB_LEN` (256 GiB) upfront while physical disk pages back
  lazily as data is written
* Allocations track 16-byte aligned units via bitmasks, avoiding external fragmentation
* Backing files grow dynamically in uniform OS page increments, minimizing write amplification

### Operational Invariants

Constraints callers must design for:

* Key length bounded between 0x01 and 0xFF bytes (255 B)
* Maximum single-entry payload capacity is bounded by `MAX_VAL_LEN` (65,535 bytes) for key-value
  entries and list items; larger payloads return `TurboError::PayloadTooLarge`
* Multi-entry writes commit via OCC with conflict detection (retries up to 0x20 times before
  surfacing contention errors)
* Thread-safe (`Send + Sync`) within a single process via `Arc<TurboFox>`
  * Single-process exclusivity is enforced at startup via mandatory POSIX / Win32 advisory file
    locks
  * Concurrent access by multiple OS processes on the same file is strictly prohibited

## Supported Platforms

| Architecture    | Target Platform                              |
|:----------------|:---------------------------------------------|
| x86_64          | Linux, macOS, Windows                        |
| AArch64 (ARM64) | Linux, macOS, Windows                        |

## Thread Safety

`TurboFox` wraps `Arc<Engine>` internally and is thread-safe (`Send + Sync`) by default, i.e. all
point reads (`get`), writes (`set`), and deletes (`del`) run concurrently without global locks

## Benchmarks

Benchmarks execute on dedicated bare-metal cloud instances with direct NVMe block storage,

- Host instance: AWS EC2 `c6id.xlarge` (0x04 vCPUs, 0x02 physical cores,
  Intel Xeon Platinum 8375C CPU @ 2.90GHz, AVX-512 / AVX2 / SHA-NI vector extensions)
- Host memory: 0x08 GiB host DDR4 system RAM (8,153,268,224 bytes); Linux hugepages enabled at
  0x02 MiB (2,097,152 bytes)
- Storage target: Direct local NVMe SSD (`/dev/nvme1n1`, mounted at `/mnt/nvme0n1`), formatted
  under XFS (`rw,noatime,nodiratime,inode64,allocsize=65536k,logbufs=8,logbsize=256k,noquota`)
- Operating system: Linux `7.0.0-1014-aws` (x86_64)
- Operation volume: 0x10_0000 (1,048,576) uniform operations per key-value suite; scaling
  benchmarks sweep from 0x00 to 0x8000 (32,768) keys

### §1 · Concurrency Dynamics & Throughput Scaling

Throughput evaluations measure sustained operations per second comparing single-thread execution
against concurrent worker transactions across 0x10_0000 operations; workloads evaluate mutation
writes (`SET` with 80% new keys and 20% updates), point lookups (`GET` with 60% hits and 40%
misses), and key deletions (`DEL` with 60% present keys and 40% missing)

<img src="assets/kv_throughput_concurrency.svg" width="100%"
  alt="KV Operation Throughput Under Concurrency" />

Point reads and deletions execute with sub-microsecond internal dispatch; because point reads are
lock-free and execute directly against memory-mapped pages without shared mutex acquisition,
concurrency overhead on read throughput is dominated by CPU L1/L2 cache coherency traffic and memory
bus contention between physical cores rather than engine-level synchronization barriers

<img src="assets/kv_throughput_log_concurrency.svg" width="100%"
  alt="Operation Throughput Logarithmic Scale" />

The continuous logarithmic throughput profile illustrates the architectural divide between
in-memory dispatch and persistent storage commits; point reads and deletes operate across the
mega-ops spectrum while writes track the persistent NVMe barrier regime

<img src="assets/kv_concurrency_scaling_efficiency.svg" width="100%"
  alt="Concurrency Scaling Efficiency" />

In contrast, mutation writes (`SET`) isolate write lock and transaction commit contention,

* Single-thread writes commit sequentially into memory pages with minimal OCC transaction overhead
* Concurrent multi-transaction writes validate optimistic concurrency control (OCC) epochs,
  serialize atomic commit tickets, and synchronize page-level modifications; competing writers
  incur OCC retry cycles and page writeback delays under concurrent transaction pressure

<img src="assets/kv_wall_clock_duration.svg" width="100%"
  alt="Total Execution Wall Time" />

Translating throughput into real-world batch latency reveals the operational cost of NVMe commit
synchronization; batch point reads complete with sub-second execution, whereas persistent write
batches reflect the physical latency of dirty page flushing and OCC retry backoff

> [!NOTE]
> Point lookups and deletions execute lock-free via memory-mapped pages without shared locks; read
> scaling degradation is bounded by hardware CPU cache coherency and memory bus bandwidth across
> physical cores rather than engine-level synchronization barriers

### §2 · Tail Latency Spectrum & Percentile CDF

Latency distributions capture response times from median (`p50`) through extreme tail (`p99`,
`p999`, and `max`), isolating where lock contention and NVMe persistence barriers impact callers

<img src="assets/kv_tail_latency_split_threads.svg" width="100%"
  alt="Tail Latency Distribution" />

Read and delete operations exhibit near-flat percentile curves across the entire distribution:

* Point lookups (`GET`): Median latency remains sub-microsecond; at higher percentiles up to `p99`,
  latencies stay tightly bounded under lock-free memory mapping
* Key deletions (`DEL`): Deletions execute in place with bounded percentile progression, reclaiming
  slots without triggering page table re-allocation
* Zero heap allocation on miss: Missing keys resolve via hash routing without traversing payload
  pages, avoiding memory allocator pressure and eliminating GC-style tail pauses

<img src="assets/kv_tail_latency_grouped_bars.svg" width="100%"
  alt="Tail Latency Grouped Bars Comparison" />

Direct head-to-head comparisons across percentile SLA tiers illustrate exact contention divergence;
while median latencies across workloads remain tight, write tail latencies expand at higher tiers
under multi-threaded contention due to OCC validation conflicts and NVMe writeback serialization

<img src="assets/kv_latency_cdf_curves.svg" width="100%"
  alt="Latency Percentile Progression Curves" />

The semi-log percentile progression curves highlight the sharp inflection point for write mutations:

* Sub-microsecond median write: Sequential writes commit into active memory pages with low median
  latency
* NVMe writeback & OCC retry tail: Latency diverges past the upper percentiles, reflecting the cost
  of kernel dirty page flushing and OCC conflict aborts under heavy mutation pressure

> [!TIP]
> Sub-microsecond median dispatch holds across lock-free point operations; zero heap allocations on
> key-miss paths guarantee flat tail profiles across percentile tiers

### §3 · Memory Residency & Resident Set Size (RSS)

Peak resident set size (RSS) tracks actual physical memory consumed by host operating system page
tables and internal transaction state after completing 0x10_0000 operations

<img src="assets/kv_memory_footprint_concurrency.svg" width="100%"
  alt="Resident Memory Footprint Under Concurrency" />

Physical storage growth on NVMe tracks sequential allocation while memory footprint remains bounded:

* Demand-paged single thread: Single-worker transactions maintain a resident memory footprint well
  below on-disk database size because unreferenced pages remain un-faulted by the kernel
* Concurrent worker buffers: Under concurrent worker transactions, resident RAM expands to back
  thread-local transaction commit logs, OCC validation buffers, and active memory-mapped pages
  faulted across distinct worker threads

> [!NOTE]
> Memory-mapped persistence utilizes OS page tables for on-demand caching; resident memory stays
> bounded by active working set size rather than total container file volume

### §4 · Cold-Open Latency & Index Memory Density

Initialization benchmarks evaluate cold database startup duration and memory scaling as stored key
volume grows from 0x00 to 0x8000 (32,768) keys; configurations contrast raw initialization against
eager in-memory lookup cache construction

<img src="assets/scale_perf_combined_horizontal.svg" width="100%"
  alt="Database Open Latency and Resource Footprint Scaling" />

Cold startup behavior exhibits two distinct operational modes:

* Raw database open (`lookup_cache_disabled`): Bypasses key traversal entirely; startup latency
  remains sub-millisecond and flat; resident memory footprint remains pinned at a constant 4 KiB
  (0x1000 bytes) regardless of database size, as backing storage pages are mapped into virtual
  address space without being faulted into physical RAM
* Cached database open (`lookup_cache_enabled`): Traverses on-disk hash buckets at startup to
  pre-warm an in-memory hash index; initialization duration scales linearly with key count; this
  upfront cost amortizes lookup latencies for read-intensive workloads by routing subsequent reads
  in memory without disk navigation

<img src="assets/scale_perf_combined_vertical.svg" width="100%"
  alt="Database Open Latency and Resource Footprint Scaling Aligned" />

Aligning open duration directly against backing resource consumption over a shared key axis reveals
the proportional relationship between disk allocation and cache latency; physical disk space grows
monotonically without external fragmentation, while cached open latency tracks hash bucket depth
without experiencing non-linear compaction spikes

<img src="assets/scale_perf_cache_overhead.svg" width="100%"
  alt="In-Memory Lookup Cache Overhead and RAM Density" />

The cache overhead profile illustrates the operational trade-offs of eager indexing:

* Latency multiplier: The startup time ratio between cached and raw open remains bounded while cold
  open stays sub-millisecond across tested intervals
* Memory density: The in-memory lookup cache requires only a few bytes of physical RAM per indexed
  key, ensuring predictable memory budgets in embedded systems and edge runtime environments
* Monotonic storage growth: Backing storage expands monotonically in uniform OS page increments
  without allocation fragmentation or stop-the-world compaction cycles

> [!TIP]
> Uncached database open incurs zero heap allocation overhead, preserving a 4 KiB resident memory
> footprint across any key scale while keeping cold initialization strictly bounded

## Contributions

Contributions are welcome ;)

please feel free to submit a PR or open an issue if you have any feedback or suggestions
