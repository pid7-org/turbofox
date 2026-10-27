[![Crates.io](https://img.shields.io/crates/v/turbofox?style=flat-square&logo=rust&logoColor=eff1f5&color=1e66f5&labelColor=4c4f69)](https://crates.io/crates/turbofox)
[![Docs.rs](https://img.shields.io/docsrs/turbofox?style=flat-square&logo=docs.rs&logoColor=eff1f5&color=179299&labelColor=4c4f69)](https://docs.rs/turbofox)
[![Tests](https://img.shields.io/github/actions/workflow/status/pid7-org/turbofox/tests.yaml?style=flat-square&logo=github&logoColor=eff1f5&label=tests&color=40a02b&labelColor=4c4f69)](https://github.com/pid7-org/turbofox/actions/workflows/tests.yaml)
[![MSRV](https://img.shields.io/badge/MSRV-1.86.0-fe640b?style=flat-square&logo=rust&logoColor=eff1f5&labelColor=4c4f69)](#usage)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-8839ef?style=flat-square&logo=open-source-initiative&logoColor=eff1f5&labelColor=4c4f69)](LICENSE-MIT)

# TurboFox 🦊🚀

An embedded, memory-mapped key-value engine providing lock-free concurrency and explicit sync
barriers

> [!IMPORTANT]
> `turbofox` is under active development and past the initial MVP phase, but is not yet reliable
>
> Architecture and disk layout remain subject to breaking changes; production use is discouraged,
> but not prohibited, given _you assume all the risks_

## Index

- [Usage](#usage)
- [Architecture Guarantees](#architecture-guarantees)
  - [Zero Rehash](#zero-rehash)
  - [Torn-Write Protection](#torn-write-protection)
  - [Durability Model](#durability-model)
  - [TurboList](#turbolist)
  - [Storage Topography](#storage-topography)
  - [Operational Invariants](#operational-invariants)
- [Supported Platforms](#supported-platforms)
- [Thread Safety](#thread-safety)
- [API](#api)
- [Benchmarks](#benchmarks)
  - [Evaluation Environment](#evaluation-environment)
  - [Operation Throughput & Concurrency Scaling](#operation-throughput--concurrency-scaling)
  - [Tail Latency Profile Under Concurrency](#tail-latency-profile-under-concurrency)

## Usage

Add `turbofox` to your `Cargo.toml`:

```toml
[dependencies]
turbofox = "0.0.101"
```

> [!NOTE]
> Minimum Supported Rust Version (MSRV) is `1.86.0`

### Quickstart

```rust
use turbofox::{TurboCfg, TurboFox};

let dir = tempfile::tempdir().unwrap();
let path = dir.path();

let cfg = TurboCfg::manual(path);
let db = TurboFox::create(cfg).unwrap();

let ticket = db.set(b"device:01", b"state_active").unwrap();
assert_eq!(db.get(b"device:01").unwrap(), Some(b"state_active".to_vec()));

db.sync().unwrap();
assert!(ticket.is_durable());

assert!(db.del(b"device:01").unwrap());
assert_eq!(db.get(b"device:01").unwrap(), None);

let list = db.list(b"events");
list.append(b"boot").unwrap();
list.append(b"init").unwrap();
assert_eq!(list.len().unwrap(), 2);
assert_eq!(list.pop().unwrap(), Some(b"init".to_vec()));

drop(list);
db.delete().unwrap();
```

## Architecture Guarantees

TurboFox is an embedded key-value storage engine engineered around memory-mapped persistence,
optimistic concurrency control, and zero-rehash index chaining

Low-level reference documentation and method signatures are available on
[docs.rs](https://docs.rs/turbofox)

### Zero Rehash

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

Mutations (`set`, `del`, `append`, `pop`) commit immediately via optimistic concurrency control and
return a lightweight `TurboTicket` bound to a monotonic commit epoch:

* Decouples memory-speed writes from physical NVMe writeback latency, where durability guarantees
  depend on the configured sync strategy
* Manual Mode (`sync_interval: None`):
  * The engine performs no automatic background flushing
  * The caller must invoke `db.sync()` or block on `ticket.wait()` to guarantee crash safety on disk
  * Un-flushed mutations remain volatile against OS kernel panics or host power loss
* Periodic Sync Mode (`sync_interval: Some(dur)`):
  * A dedicated background sync thread flushes dirty pages up to the current epoch
  * Incorporates adaptive backoff under idle load to prevent write endurance wear on flash devices
* `ticket.is_durable()` enables non-blocking checks to poll writeback completion asynchronously

### TurboList

Ordered 1-to-N sequences co-exist inside the primary storage file:

* Eliminates secondary index tables, auxiliary catalog files, and relational join overhead
* Provides ordered, append-only lists with amortized O(1) appends and pops
* Supports fast indexed random access (`get_at`), boundary inspection (`first`, `last`), and
  snapshot iteration (`iter`)
* Validates item integrity via CRC32 checksums on every read

### Storage Topography

Unified single-file storage container managed across memory-mapped virtual address space:

* Initial file footprint begins at only 16 KiB on standard host systems
* Virtual memory space reserves up to `MAX_DB_LEN` (512 GiB) upfront while physical disk pages back
  lazily as data is written
* Allocations track 16-byte aligned units via bitmasks, avoiding external fragmentation
* Backing files grow dynamically in uniform OS page increments, minimizing write amplification

### Operational Invariants

Constraints callers must design for:

* Key length bounded between 0x01 and 0xFF bytes (255 B)
* Maximum single-entry payload capacity is 1004 bytes for key-value entries and 1002 bytes for list
  items; larger payloads return `TurboError::PayloadTooLarge`
* Multi-entry writes commit via OCC with conflict detection (retries up to 0x20 times before
  surfacing contention errors)
* Thread-safe (`Send + Sync`) within a single process via `Arc<TurboFox>`
  * Single-process exclusivity is enforced at startup via mandatory POSIX / Win32 advisory file
    locks
  * Concurrent access by multiple OS processes on the same file is strictly prohibited

## Supported Platforms

| Architecture    | Target Platform                              | Storage Access      |
|:----------------|:---------------------------------------------|:--------------------|
| x86_64          | Linux, macOS, Windows                        | POSIX / Win32 mmap  |
| AArch64 (ARM64) | Linux, macOS, Windows                        | POSIX / Win32 mmap  |

## Thread Safety

`TurboFox` wraps `Arc<Engine>` internally and is thread-safe (`Send + Sync`) by default, i.e. all
point reads (`get`), writes (`set`), and deletes (`del`) run concurrently without global locks

## API

Refer to [docs.rs](https://docs.rs/turbofox/latest/turbofox/) for crate documentation and full API
references

## Benchmarks

### Evaluation Environment

Benchmarks execute on dedicated cloud instances with direct NVMe storage:

- Instance: AWS EC2 `c5d.large` (2 vCPUs, Intel Xeon Platinum 8223CL CPU @ 3.00 GHz)
- Memory: 0x04 GiB DDR4 system RAM
- Storage: Dedicated local hardware NVMe SSD (`/mnt/nvme0n1`)
- Workload: 0x186A0 (100,000) operations per benchmark suite under uniform distribution
- Concurrency: 1-thread baseline vs 4-thread concurrent worker evaluation

### Operation Throughput & Concurrency Scaling

Sustained key-value operations per second across single-thread and 4-thread worker configurations

<img src="assets/kv_throughput_scaling.svg" width="100%" alt="KV Throughput Scaling" />

| Workload              | 1 Thread Throughput | 4 Thread Throughput | Scaling Ratio |
|:----------------------|:--------------------|:--------------------|:--------------|
| SET (Insert)          | 1.18 kops/s         | 971 ops/s           | 0.82x         |
| GET (60/40 Hit/Miss)  | 1.89 kops/s         | 1.86 kops/s         | 0.98x         |
| DEL (60/40 Hit/Miss)  | 1.89 kops/s         | 1.85 kops/s         | 0.98x         |

> [!NOTE]
> Evaluation executes on 2 vCPU hardware where 4 worker threads oversubscribe physical compute cores;
> throughput scaling reflects compute core oversubscription and OCC slot contention

### Tail Latency Profile Under Concurrency

Percentile latency distributions (p50 to p99.9) comparing 1T baseline against 4T concurrent workers

<img src="assets/kv_tail_latency.svg" width="100%" alt="KV Tail Latency Profile" />

| Workload              | Threads | p50     | p90     | p99     | p99.9     |
|:----------------------|:--------|:--------|:--------|:--------|:----------|
| SET: Insert           | 1T      | 0.80 ms | 1.41 ms | 3.11 ms | 4.06 ms   |
| SET: Insert           | 4T      | 0.96 ms | 1.92 ms | 4.59 ms | 420.48 ms |
| GET: 60/40 Hit/Miss   | 1T      | 0.29 ms | 1.47 ms | 1.72 ms | 1.75 ms   |
| GET: 60/40 Hit/Miss   | 4T      | 0.64 ms | 4.28 ms | 7.24 ms | 7.39 ms   |
| DEL: 60/40 Hit/Miss   | 1T      | 0.29 ms | 1.46 ms | 1.72 ms | 1.75 ms   |
| DEL: 60/40 Hit/Miss   | 4T      | 0.64 ms | 4.18 ms | 7.24 ms | 7.38 ms   |

> [!NOTE]
> Sub-millisecond median latencies (p50 < 0.30 ms for reads and deletes) sustained under 1T baseline
>
> Read and delete tail latencies stay bounded under 7.5 ms at p99.9 under 4T concurrency while
> elevated 4T p99.9 write tail stems from OCC conflict retries and backing file growth serialization

## Contributions

Contributions are welcome ;)

please feel free to submit a PR or open an issue if you have any feedback or suggestions
