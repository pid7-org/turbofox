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

TurboFox is an embedded key-value storage engine engineered around memory-mapped slot topography,
optimistic concurrency control, and zero-rehash overflow chaining

Low-level reference documentation and method signatures are available on
[docs.rs](https://docs.rs/turbofox)

### Zero Rehash

Primary key routing utilizes 64-bit Wyhash bit-sliced into depth-indexed buckets (64 slots each)

Hash collisions resolve via a localized 4-slot probing window followed by overflow bucket chaining,

* Entries are permanently anchored to their allocated bucket chain and never rehashed across files,
  which eliminates stop-the-world hash table redistribution pauses while sustaining bounded p99
  write latency
* In-place overwrites execute directly within preallocated DataBuf spans when new payloads fit in
  existing 16-byte chunk boundaries
* Structural allocations expand incrementally via monotonic bump allocation w/o bulk page copies

### Torn-Write Protection

Point reads execute lock-free w/o reader-writer locks or shared mutex acquisition

Readers probe 64-bit hash sentinels directly within mapped virtual memory cache lines,

* Payloads utilize binary framing: `[ CRC32 (4 B) ][ Key ][ Val ][ Pad ]`
* Reading validates CRC32 integrity alongside byte-level key equality (concurrent writes or partial
  page flushes that cause torn reads fail safely via `TurboError::CorruptedPayload` rather than
  returning corrupt data)
- Negative queries (`Ok(None)`) short-circuit on zero-sentinels w/o accessing payload pages (zero
  heap allocations occur during probe traversal and miss evaluations)

### Durability Model

Mutations (`set`, `del`, `append`, `pop`) commit into memory-mapped slots immediately via OCC and
return a lightweight `TurboTicket` bound to a monotonic commit epoch,

* Decouples memory-speed writes from physical NVMe writeback latency, where durability guarantees
  depend on the configured sync strategy
* Manual Mode (`sync_interval: None`),
  * The engine performs no automatic background flushing
  * The caller must invoke `db.sync()` or block on `ticket.wait()` to guarantee crash safety on disk
  * Un-flushed mutations remain volatile against OS kernel panics or host power loss
* Periodic Sync Mode (`sync_interval: Some(dur)`),
  * A dedicated background sync thread flushes dirty pages up to the current epoch
  * Incorporates adaptive backoff under idle load to prevent write endurance wear on flash devices
* `ticket.is_durable()` enables non-blocking checks to poll writeback completion asynchronously

### TurboList

Ordered 1-to-N sequences co-exist inside the primary storage file

Anchored by a primary key registered in root buckets w/ type flag `offsets[i][0] & 0x80 != 0`,

* Eliminates secondary index tables, auxiliary catalog files, and relational join overhead
* Sequences chain through compact 20-byte `AdjArrayList` descriptor blocks (4 item slots per node)
* Tail appends sustain amortized O(1) latency (pops deallocate item DataBuf spans and return empty
  descriptor blocks to allocation bitmaps)
* Readers traverse chained descriptors w/ index-based skipping (`idx / 0x04`) and CRC32 checks

### Storage Topography

Unified storage container structured into uniform 128-byte slots

Initial file footprint begins at only 16 KiB (0x80 slots) on standard 4 KiB host page systems,

* Virtual memory space reserves up to `MAX_DB_LEN` upfront (physical disk pages back lazily as
  monotonic bump-pointer slot markers advance)
* Buffer pools track 16-byte chunks via 64-bit bitmaps, eliminating internal space fragmentation
* Backing files grow dynamically in uniform OS page increments, minimizing write amplification

### Operational Invariants

Hardware and protocol constraints callers must design for,

* Key length bounded between 0x01 and 0xFF bytes and value payload size capped at 0xFFFF bytes
* Contiguous buffer spans within standard DataBuf blocks cap at 0x40 buffers (1024 bytes) while
  larger payloads span chained segment buffers
* Multi-slot writes commit via lock-free OCC w/ conflict detection across mapped memory slots
  (aborts and retries up to 0x20 times before surfacing contention errors)
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
point reads (`get`), writes (`set`), and deletes (`del`) run concurrently w/o global locks

## API

Refer to [docs.rs](https://docs.rs/turbofox/latest/turbofox/) for crate documentation and full API
references

## Benchmarks

### Evaluation Environment

Benchmarks execute on bare-metal virtualized cloud instances w/ direct NVMe storage:

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
| SET (Insert)          | 1.18 kops/s         | 963 ops/s           | 0.82x         |
| GET (60/40 Hit/Miss)  | 1.95 kops/s         | 1.78 kops/s         | 0.91x         |
| DEL (60/40 Hit/Miss)  | 1.95 kops/s         | 1.78 kops/s         | 0.91x         |

> [!NOTE]
> Evaluation executes on 2 vCPU hardware where 4 worker threads oversubscribe physical compute cores;
> throughput scaling reflects compute core oversubscription and OCC slot contention

### Tail Latency Profile Under Concurrency

Percentile latency distributions (p50 to p99.9) comparing 1T baseline against 4T concurrent workers

<img src="assets/kv_tail_latency.svg" width="100%" alt="KV Tail Latency Profile" />

| Workload              | Threads | p50     | p90     | p99     | p99.9     |
|:----------------------|:--------|:--------|:--------|:--------|:----------|
| SET: Insert           | 1T      | 0.81 ms | 1.42 ms | 3.11 ms | 3.99 ms   |
| SET: Insert           | 4T      | 0.96 ms | 1.94 ms | 4.57 ms | 422.58 ms |
| GET: 60/40 Hit/Miss   | 1T      | 0.28 ms | 1.42 ms | 1.68 ms | 1.71 ms   |
| GET: 60/40 Hit/Miss   | 4T      | 0.62 ms | 4.69 ms | 8.04 ms | 10.45 ms  |
| DEL: 60/40 Hit/Miss   | 1T      | 0.29 ms | 1.42 ms | 1.68 ms | 1.71 ms   |
| DEL: 60/40 Hit/Miss   | 4T      | 0.61 ms | 4.61 ms | 7.60 ms | 10.35 ms  |

> [!NOTE]
> Sub-millisecond median latencies (p50 < 0.30 ms for reads and deletes) sustained under 1T baseline
>
> Read and delete tail latencies stay bounded under 10.5 ms at p99.9 under 4T concurrency while
> elevated 4T p99.9 write tail stems from OCC conflict retries and backing file growth serialization

## Contributions

Contributions are welcome ;)

please feel free to submit a PR or open an issue if you have any feedback or suggestions
