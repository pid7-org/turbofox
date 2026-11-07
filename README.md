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
  - [Evaluation Environment](#evaluation-environment)
  - [Operation Throughput & Concurrency Scaling](#operation-throughput--concurrency-scaling)
  - [Tail Latency Profile Under Concurrency](#tail-latency-profile-under-concurrency)
  - [Database Open Latency Scaling](#database-open-latency-scaling)
  - [Resource Footprint Scaling](#resource-footprint-scaling)

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

### Evaluation Environment

Benchmarks execute on dedicated cloud instances with direct NVMe storage:

- Instance: AWS EC2 `c5d.large` (2 vCPUs, Intel Xeon Platinum 8223CL CPU @ 3.00 GHz)
- Memory: 0x04 GiB DDR4 system RAM
- Storage: Dedicated local hardware NVMe SSD (`/mnt/nvme0n1`)
- Kernel: Linux `7.0.0-1014-aws` (x86_64)
- Workload: 0x100_000 (1,048,576) operations per benchmark suite under uniform distribution

### Operation Throughput & Concurrency Scaling

Sustained key-value operations per second across single-thread worker configuration

<img src="assets/kv_throughput_scaling.svg" width="100%" alt="KV Throughput Scaling" />

| Workload               | Throughput    |
|:-----------------------|:--------------|
| SET (80/20 New/Update) | 26.12 kops/s  |
| GET (60/40 Hit/Miss)   | 1.07 Mops/s   |
| DEL (60/40 Hit/Miss)   | 827.00 kops/s |

### Tail Latency Profile Under Concurrency

Percentile latency distributions (p50 to p9999)

<img src="assets/kv_tail_latency.svg" width="100%" alt="KV Tail Latency Profile" />

| Workload              | p50     | p90     | p99       | p9999       |
|:----------------------|:--------|:--------|:----------|:------------|
| SET: 80/20 New/Update | 1.42 µs | 1.76 µs | _1.37 ms_ | _5.03 ms_   |
| GET: 60/40 Hit/Miss   | 0.87 µs | 1.11 µs | 1.39 µs   | 6.18 µs     |
| DEL: 60/40 Hit/Miss   | 1.07 µs | 1.36 µs | 5.00 µs   | 11.30 µs    |

> [!TIP]
> Sub-1.5 µs median latencies (p50 <= 1.42 µs across all operations)

### Database Open Latency Scaling

Initialization duration across scaling key intervals comparing disabled vs enabled lookup cache

<img src="assets/scale_perf_latency.svg" width="100%" alt="Database Open Latency Scaling" />

| Scale (Entries) | Lookup Cache Disabled | Lookup Cache Enabled |
|:----------------|:----------------------|:---------------------|
| 0               | 2.89 ms               | 2.96 ms              |
| 6,553           | 2.86 ms               | 3.12 ms              |
| 13,106          | 2.86 ms               | 3.19 ms              |
| 19,659          | 2.99 ms               | 3.36 ms              |
| 26,212          | 2.87 ms               | 3.24 ms              |
| 32,765          | 3.02 ms               | 3.39 ms              |
| 39,318          | 3.02 ms               | 3.39 ms              |
| 45,871          | 2.94 ms               | 3.89 ms              |
| 52,424          | 3.02 ms               | 4.28 ms              |
| 58,977          | 3.18 ms               | 3.44 ms              |
| 65,530          | 3.58 ms               | 3.92 ms              |

> [!TIP]
> Cold-open latency remains flat (~2.9 ms to 4.3 ms) up to 0x0001_0000 entries regardless of caching

### Resource Footprint Scaling

Resident memory (RAM) and storage growth (Disk) across scaling key intervals

<img src="assets/scale_perf_footprint.svg" width="100%" alt="Resource Footprint Scaling" />

| Scale (Entries) | Disk Footprint | RAM (Cache Disabled) | RAM (Cache Enabled) |
|:----------------|:---------------|:---------------------|:--------------------|
| 0               | 16 KiB         | 4 KiB                | 24 KiB              |
| 6,553           | 816 KiB        | 4 KiB                | 72 KiB              |
| 13,106          | 1.06 MiB       | 4 KiB                | 72 KiB              |
| 19,659          | 1.20 MiB       | 4 KiB                | 72 KiB              |
| 26,212          | 1.41 MiB       | 4 KiB                | 72 KiB              |
| 32,765          | 1.70 MiB       | 4 KiB                | 72 KiB              |
| 39,318          | 2.31 MiB       | 4 KiB                | 72 KiB              |
| 45,871          | 2.97 MiB       | 4 KiB                | 72 KiB              |
| 52,424          | 3.58 MiB       | 4 KiB                | 72 KiB              |
| 58,977          | 4.17 MiB       | 4 KiB                | 72 KiB              |
| 65,530          | 4.91 MiB       | 4 KiB                | 72 KiB              |

> [!TIP]
> Zero-memory-overhead baseline retains a constant 0x04 KiB resident footprint at any key scale

## Contributions

Contributions are welcome ;)

please feel free to submit a PR or open an issue if you have any feedback or suggestions
