//! TurboFox is an embedded, memory-mapped key-value engine providing lock-free concurrency & explicit
//! sync barriers
//!
//! ## Features
//!
//! ### §1 · Point Mutations: SET & DEL
//!
//! Mutations execute via optimistic concurrency control (OCC) directly against memory-mapped pages,
//!
//! * Inserts assign slots dynamically; in-place overwrites execute without reallocation when updated
//!   payloads fit existing slot capacity
//! * Monotonic expansion bounds heap fragmentation; slots expand dynamically when updated values
//!   exceed prior allocations
//! * In-place slot reclamation via `del` marks tombstones directly without global table redistribution
//!   or stop-the-world compaction cycles
//! * Operational bounds: key length $\in [0x01, 0xFF]$ bytes (255 B); value length $\le 0xFFFF$
//!   bytes (65,535 B)
//! * OCC conflict retry loop resolves concurrent multi-threaded writes automatically (retrying up to
//!   0x20 times under contention)
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();
//!
//! // Initial write: commits slot in memory and returns commit epoch ticket
//! let ticket = db.set(b"session:0x42", b"user_active").unwrap();
//! assert!(ticket.epoch() > 0x00);
//!
//! // In-place overwrite: updates in place without allocating new storage slots
//! db.set(b"session:0x42", b"user_idle").unwrap();
//! assert_eq!(db.get(b"session:0x42").unwrap(), Some(b"user_idle".to_vec()));
//!
//! // In-place slot reclamation: frees key-value slot without table redistribution
//! assert!(db.del(b"session:0x42").unwrap());
//! assert!(!db.del(b"session:0x42").unwrap());
//!
//! db.delete().unwrap();
//! ```
//!
//! NOTE: In-place overwrites incur zero storage expansion overhead when new payload length remains
//! within previously allocated slot capacity
//!
//! ### §2 · Zero-Allocation Retrieval: GET & CONTAINS_KEY
//!
//! Point reads execute lock-free directly against memory-mapped pages without shared mutex acquisition,
//!
//! * Lock-free point lookups route through 32-bit XxHash32 bucket chains
//! * Checksum integrity verification: every payload read validates CRC32 integrity alongside
//!   byte-level key equality; torn reads from concurrent writes fail safely via
//!   [`TurboError::CorruptedPayload`]
//! * Zero-allocation key presence probe: `contains_key` probes bucket presence without copying or
//!   allocating value payload buffers on the heap
//! * Lookup misses short-circuit at bucket index level without touching data payload pages; zero
//!   heap allocations occur on non-existent keys
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();
//!
//! db.set(b"device:sensor_01", b"temp=21.4C;humidity=45%").unwrap();
//!
//! // Zero-allocation existence probe: resolves without heap Vec allocations
//! assert!(db.contains_key(b"device:sensor_01").unwrap());
//! assert!(!db.contains_key(b"device:sensor_99").unwrap());
//!
//! // Value retrieval: verifies CRC32 checksum before copying payload bytes
//! let payload = db.get(b"device:sensor_01").unwrap();
//! assert_eq!(payload, Some(b"temp=21.4C;humidity=45%".to_vec()));
//!
//! db.delete().unwrap();
//! ```
//!
//! TIP: Prefer `contains_key` over `get` whenever existence verification does not consume payload
//! bytes; probing inspects index headers directly, avoiding heap allocations entirely
//!
//! ### §3 · Durability Pipeline: TurboTicket & Sync Barriers
//!
//! Decouples high-frequency in-memory mutation throughput from physical NVMe writeback latency,
//!
//! * Monotonic epoch tracking: mutations (`set`, `del`, `append`, `pop`) return a lightweight
//!   [`TurboTicket`] bound to a strictly increasing commit epoch
//! * Non-blocking verification: `ticket.is_durable()` checks hardware writeback status atomically
//!   without parking the calling thread
//! * Cumulative barrier invariant: physical flushes advance the durable epoch monotonically;
//!   establishing a barrier for epoch $E$ guarantees all mutations with epochs $\le E$ are persisted
//!   to disk
//! * Flexible durability policies,
//!   * *Per-Write Sync*: call `ticket.force()` immediately for strict hardware durability at each write
//!   * *Batched Epoch Sync*: execute multi-mutation batches in memory at multi-million ops/sec, then
//!     issue a single barrier on the final ticket to persist the entire batch simultaneously
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();
//!
//! // Pattern A · Strict Per-Write Sync (~26 kops/s on NVMe)
//! // Forces physical msync on every mutation; throughput bounded by SSD writeback latency
//! for i in 0..0x0A {
//!     let key = format!("strict:0x{:x}", i);
//!     let ticket = db.set(key.as_bytes(), b"state").unwrap();
//!     ticket.force().unwrap(); // Blocks calling thread until persisted on NVMe
//! }
//!
//! // Pattern B · Batched Epoch Sync (~1+ Mops/s throughput)
//! // Mutations commit to memory pages at memory bus speeds;
//! // Calling force() on the final ticket commits all preceding mutations in a single hardware barrier
//! let mut last_ticket = None;
//! for i in 0..0x03E8 {
//!     let key = format!("batch:0x{:x}", i);
//!     last_ticket = Some(db.set(key.as_bytes(), b"metric").unwrap());
//! }
//!
//! if let Some(ticket) = last_ticket {
//!     ticket.force().unwrap(); // Single sync barrier persists all 0x03E8 mutations
//!     assert!(ticket.is_durable());
//! }
//!
//! db.delete().unwrap();
//! ```
//!
//! IMPORTANT: Durability barriers are epoch-cumulative; calling `ticket.force()`, `ticket.wait()`,
//! or `db.sync()` on ticket $N$ atomically persists all pending mutations $0..N$ in one physical
//! flush
//!
//! ### §4 · Embedded Sequences: TurboList
//!
//! Ordered 1-to-N sequences co-located inside the primary container file without auxiliary tables or
//! relational join overhead,
//!
//! * Sequential appends and pops execute with amortized O(1) complexity
//! * Boundary inspection via `first` and `last` inspects queue extremes without traversing elements
//! * Zero-based random access via `get_at` fetches items by ordinal position
//! * Demand-paged forward iteration via `iter` streams active items with O(1) payload footprint
//! * Pre-read slot validation terminates iteration cleanly if trailing items are popped
//! * Per-item CRC32 checksums validate payload integrity on every retrieval
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();
//!
//! let list = db.list(b"jobs:stream");
//!
//! // Amortized O(1) sequential appends into memory-mapped contiguous arrays
//! list.append(b"job_1").unwrap();
//! list.append(b"job_2").unwrap();
//! list.append(b"job_3").unwrap();
//! assert_eq!(list.len().unwrap(), 0x03);
//!
//! // Boundary probes without traversing the underlying array
//! assert_eq!(list.first().unwrap(), Some(b"job_1".to_vec()));
//! assert_eq!(list.last().unwrap(), Some(b"job_3".to_vec()));
//!
//! // O(1) indexed random access by 0-based position
//! assert_eq!(list.get_at(0x01).unwrap(), Some(b"job_2".to_vec()));
//!
//! // Demand-paged forward iterator verifying per-item CRC32 integrity
//! let items: Vec<_> = list.iter().unwrap().collect();
//! assert_eq!(items.len(), 0x03);
//!
//! // Stack/queue pop and slot reclamation
//! assert_eq!(list.pop().unwrap(), Some(b"job_3".to_vec()));
//! assert!(list.clear().unwrap());
//! assert!(list.is_empty().unwrap());
//!
//! drop(list);
//! db.delete().unwrap();
//! ```
//!
//! NOTE: `TurboList` operations share memory-mapped adjacent array descriptor blocks; calling
//! `clear()` reclaims descriptor slots in place without file truncation or storage reorganization;
//! `iter()` snapshots compact 12 B index descriptors upfront and validates slot occupancy before
//! reading payloads, halting cleanly without reading recycled memory if concurrent pops occur
//!
//! ### §5 · Configuration & Lifecycle: TurboCfg
//!
//! Engine initialization, sync policies, and storage pre-allocation parameters,
//!
//! * Durability policies,
//!   * `SyncMode::Manual`: caller retains explicit control over flush timing and background threads
//!   * `SyncMode::Periodic(Duration)`: spawns a background thread coalescing dirty pages at interval
//! * Upfront storage sizing via `CapacityHint`: pre-allocates backing database files to avoid
//!   runtime `mmap` expansion latency spikes during high-throughput ingestion
//! * Lookup cache mode: trade-off between instant cold startup (4 KiB RSS) and pre-indexed memory
//!   lookups for read-intensive workloads
//!
//! ```
//! use std::time::Duration;
//! use turbofox::{CapacityHint, SyncMode, TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//!
//! // Pre-size container upfront to eliminate runtime mmap expansion pauses
//! let hint = CapacityHint::new()
//!     .with_kv(0x0001_86A0, 0x10, 0x20)
//!     .with_list(0x0400, 0x08, 0x40);
//!
//! // Manual sync mode: caller retains total control over flush timing and threads
//! let manual_cfg = TurboCfg::manual(path).with_capacity_hint(hint);
//! let db = TurboFox::open_or_create(manual_cfg).unwrap();
//!
//! // Periodic sync mode: dedicated background worker flushes at interval
//! let periodic_cfg = TurboCfg::periodic(path, Duration::from_millis(0x32));
//! assert_eq!(periodic_cfg.sync_mode, SyncMode::Periodic(Duration::from_millis(0x32)));
//!
//! db.delete().unwrap();
//! ```
//!
//! TurboFox provides three distinct initialization entry points to match operational requirements,
//!
//! | Method           | Target Exists | Target Missing | Target Corrupt | Recommended Use Case        |
//! |:-----------------|:--------------|:---------------|:---------------|:----------------------------|
//! | `open_or_create` | Opens DB      | Formats DB     | Fails (`Err`)  | Application boot sequences  |
//! | `create`         | Fails (`Err`) | Formats DB     | N/A            | Provisioning & cold setups  |
//! | `open`           | Opens DB      | Fails (`Err`)  | Fails (`Err`)  | Strict crash restarts       |
//!
//! TIP: Use `CapacityHint` in write-heavy environments to pre-size storage pages upfront,
//! eliminating runtime memory-map resizing and bounding tail write latency
//!
//! ### §6 · Page Cache Eviction & Telemetry: evict
//!
//! Granular control over OS page cache residency and backing storage footprint,
//!
//! * Advisory cache trimming via `evict`: advises host kernel page tables to release clean file-backed
//!   pages to relieve physical RAM pressure
//! * `EvictionMode::PreserveHeaders`: unmaps data pages while retaining index headers hot in RAM
//! * `EvictionMode::PurgeAll`: unmaps all database pages across virtual address space; subsequent
//!   reads demand-page faulted blocks back into physical RAM
//! * Automatic pre-eviction sync: flushes dirty pages before dispatching kernel eviction hints,
//!   ensuring crash consistency and marking outstanding tickets durable
//! * Telemetry metrics: inspect active page residency via `resident_bytes_in_memory()` and file
//!   allocation via `resident_bytes_on_disk()`
//!
//! ```
//! use turbofox::{EvictionMode, TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let db = TurboFox::create(TurboCfg::manual(dir.path())).unwrap();
//!
//! db.set(b"hot_key", b"payload_bytes").unwrap();
//!
//! // Telemetry: inspect physical memory residency vs disk allocation
//! assert!(db.resident_bytes_in_memory().unwrap() > 0x00);
//! assert!(db.resident_bytes_on_disk() >= 0x4000); // 16 KiB initial footprint
//!
//! // Pre-eviction sync flushes dirty pages automatically;
//! // Sheds cold data pages from physical RAM while keeping index headers hot
//! db.evict(EvictionMode::PreserveHeaders).unwrap();
//!
//! // Subsequent read faults evicted page back into physical memory on demand
//! assert_eq!(db.get(b"hot_key").unwrap(), Some(b"payload_bytes".to_vec()));
//!
//! db.delete().unwrap();
//! ```
//!
//! ## Architecture Guarantees
//!
//! TurboFox provides deterministic guarantees across storage layouts, index routing, concurrent
//! execution, and physical media writeback
//!
//! ### §1 · Zero Global Rehash & 15-Way Prefix Folding
//!
//! Primary routing is governed by an embedded 15-way radix trie backed by 32-bit `XxHash32`,
//!
//! * Hash Bit-Slicing & Folding: key hashes decompose into 4-bit slices across eight depth levels
//!   (`depth % 8`); slice values $S \in [0, 15]$ fold $15 \to 0$ to address 15 child branches
//!   without integer division
//! * Leaf Bucket Topography: leaf nodes terminate in 256-byte Buckets (4 slots) holding 32 entry
//!   hashes and 32 buffer offsets; candidate hashes are scanned in parallel via SIMD vector
//!   instructions (AVX-512 / AVX2 / SSE2 / NEON)
//! * Localized Bucket Splitting: when a bucket exceeds 32 entries at `depth < 0x08`, the engine
//!   allocates 14 new Buckets and 1 routing Node from `free_slot_marker`; entries with
//!   `child_idx == 0` are compacted in place, while entries $1..14$ route into new child Buckets
//! * Atomic Pointer Publication: the new Node is published to the parent via an atomic optimistic
//!   pointer swap, transitioning readers instantly without write-ahead logs or intermediate states
//! * Bounded Tail Latency: bucket splits are strictly localized to 14 child buckets; global table
//!   redistributions and stop-the-world compaction cycles do not exist
//!
//! NOTE: Hash collisions at `MAX_TRIE_DEPTH` (0x08) return `TurboError::Storage` rather than
//! entering unbounded recursive allocations, preserving predictable memory bounds
//!
//! ### §2 · Torn-Write Immunity & Checksum Verification
//!
//! Point lookups execute lock-free directly against memory-mapped virtual address space,
//!
//! * Lock-Free Point Reads: point readers bypass read-write locks, spinlocks, and shared reference
//!   counters; reads navigate mapped pages without generating CPU cache coherency traffic
//! * CRC32 Payload Framing: every stored key-value and list item prepends an 8-byte framing descriptor
//!   containing an endian-independent CRC32 checksum, key length (`klen`), and payload length (`vlen`)
//! * Fast 2-Buffer Collision Filtering: buffer 0 loads header framing, buffer 1 loads the initial
//!   8-byte key prefix; if `klen` or key prefix mismatches, hash collisions abort immediately
//!   without touching payload pages
//! * Safe Torn-Read Rejection: concurrent writes or partial page writebacks that cause torn reads fail
//!   safely via `TurboError::CorruptedPayload` rather than surfacing corrupted state
//! * Zero Heap Allocation on Miss: non-existent keys terminate at the bucket index level; misses return
//!   `Ok(None)` with zero heap allocations and zero payload page accesses
//!
//! TIP: Lock-free reads and zero-allocation misses ensure read latency percentiles
//! remain flat and unaffected by concurrent write volume
//!
//! ### §3 · Optimistic Concurrency Control & Deadlock Freedom
//!
//! Mutations execute through atomic multi-slot transactions governed by optimistic concurrency control,
//!
//! * Slot-Level Granularity: transactions operate on discrete 64-byte `Slot` boundaries;
//!   writers compute mutations in private memory before staging slot modifications
//! * Deadlock Prevention Guarantee: target slot indices are sorted in strictly ascending numerical
//!   order before lock acquisition; ordered acquisition mathematically eliminates circular wait
//!   conditions, guaranteeing deadlock-free execution
//! * Atomic Conflict Validation: commits verify that target slots match expected pre-state before
//!   applying updates; on conflict, locks release immediately, and the writer retries with randomized
//!   exponential backoff ($50\,\mu\text{s}..10\,\text{ms}, \pm 25\%$ jitter) up to 0x20 cycles
//! * Non-Blocking Telemetry Counter: active entry count (`total_entries`) is tracked in memory via
//!   `AtomicU64`, preventing writer cache-line contention on Metadata Slot 0; Slot 0 flushes during
//!   sync barriers
//!
//! NOTE: Multi-transaction write conflicts resolve through OCC backoff loops; under uncontended
//! workloads, write transactions commit in sub-microsecond intervals
//!
//! ### §4 · Storage Topography & Demand-Paged Footprint
//!
//! Unified single-file container layout mapped across virtual address space,
//!
//! * Quantized Component Geometry: all internal structures align to 8 bytes and occupy uniform 64-byte
//!   `Slot` multiples (Metadata = 1 slot, Node = 1 slot, AdjArr = 2 slots, Bucket = 4 slots,
//!   DBuf = 4 slots)
//! * Demand-Paged Virtual Mapping: reserves up to `MAX_DB_LEN` (0x40_0000_0000 bytes / 256 GiB)
//!   upfront in virtual memory; physical disk blocks back pages lazily as data is written
//! * Monotonic Slot Allocation: internal allocations advance `free_slot_marker` in 256-byte quanta
//!   (4 slots), avoiding external fragmentation
//! * Synchronized File Growth: when capacity exhausts, writers coordinate under `growth_mutex` using
//!   double-checked locking to grow the backing file in OS page increments (16 KiB on 4 KiB/16 KiB
//!   systems, 64 KiB on 64 KiB systems)
//! * Initial Storage Footprint: cold database layout occupies exactly 8 KiB (128 slots) across
//!   16 KiB initial file allocation
//!
//! NOTE: Virtual memory reservation (256 GiB) does not consume physical host RAM; physical
//! residency tracks active working set pages demand-faulted by the host kernel
//!
//! ### §5 · RAII Safety & Graceful Shutdown Protocol
//!
//! Resource management separates host OS cleanup from physical data durability,
//!
//! * RAII Resource Reclamation: dropping `TurboFox` unmaps virtual address space, releases heap
//!   allocations, closes backing file handles, and releases advisory file locks without leaks
//! * Drop Durability Boundary: Rust's `Drop` implementation initiates a best-effort flush, but
//!   discards errors; physical I/O faults cannot be surfaced during drop
//! * Explicit Shutdown Barrier: production systems performing graceful shutdown must establish an
//!   explicit sync barrier (`ticket.wait()`, `ticket.force()`, or `db.sync()`) and verify the returned
//!   `Result` prior to handle destruction
//! * Single-Process Exclusivity: enforced at startup via mandatory advisory file locks (`flock` on
//!   Unix, `LockFileEx` on Windows); opening an active database concurrently returns
//!   `TurboError::Storage`
//!
//! WARNING: Dropping a database handle without invoking `sync()` or awaiting outstanding tickets
//! risks data loss if host system power fails before the OS page cache writes dirty pages to disk
//!
//! ## Supported Platforms
//!
//! TurboFox supports 64-bit architectures across major operating systems; all target platforms are
//! validated under automated multi-platform CI pipelines,
//!
//! | Architecture | OS        | Page Size             | ISA                 |
//! |:-------------|:----------|:----------------------|:--------------------|
//! | `x86_64`     | Linux     | 4 KiB                 | AVX-512, AVX2, SSE2 |
//! | `x86_64`     | macOS     | 4 KiB                 | AVX2, SSE2          |
//! | `x86_64`     | Windows   | 4 KiB                 | AVX2, SSE2          |
//! | `aarch64`    | Linux     | 4 KiB, 16 KiB, 64 KiB | NEON                |
//! | `aarch64`    | macOS     | 16 KiB                | NEON                |
//! | `aarch64`    | Windows   | 4 KiB                 | NEON                |
//!
//! * Vector acceleration automatically detects CPU ISA features at runtime to drive SIMD hash scanning
//! * Host page sizes (4 KiB, 16 KiB, 64 KiB) are detected dynamically to align allocation growth
//!   quanta
//! * Process exclusivity is enforced across all platforms via kernel-level mandatory advisory
//!   file locks
//!
//! ## Benchmarks
//!
//! Comprehensive benchmark evaluations, throughput scaling profiles, tail latency distributions,
//! and memory footprint analyses across hardware environments are documented on
//! [GitHub](https://github.com/pid7-org/turbofox#benchmarks)

#![allow(unsafe_op_in_unsafe_fn)]
#![deny(
    missing_docs,
    trivial_casts,
    trivial_numeric_casts,
    unused_extern_crates,
    unused_import_braces,
    unused_results
)]

mod adjarr;
mod bucket;
mod cfg;
mod common;
mod dbuf;
mod engine;
mod error;
mod hash;
mod list;
mod meta;
mod node;
mod probe;
mod ticket;

pub use cfg::{CapacityHint, EvictionMode, SyncMode, TurboCfg};
pub use common::{MAX_KEY_LEN, MAX_VAL_LEN};
pub use error::{TurboError, TurboResult};
pub use list::{TurboList, TurboListIter};
pub use ticket::TurboTicket;

use engine::Engine;
use std::sync::Arc;

/// An embedded, memory-mapped key-value engine providing lock-free concurrency & explicit sync
/// barriers
///
/// Internally wraps an `Arc<Engine>`; cloning is cheap and shares the underlying memory mapping,
/// hash table, and sync worker thread pool across threads
///
/// ## Concurrency & OCC Model
///
/// Point lookups execute lock-free directly against memory-mapped pages without shared mutex
/// acquisition; mutations execute through atomic multi-slot transactions governed by optimistic
/// concurrency control (OCC) with ascending slot order acquisition to guarantee deadlock-free
/// execution
///
/// Active entry telemetry (`total_entries`) updates atomically in memory via `AtomicU64` to avoid
/// writer cache-line contention on Metadata Slot 0
///
/// ## RAII Safety
///
/// `TurboFox` is RAII-safe with respect to system resource reclamation
///
/// Dropping a database handle cleanly releases all allocated heap buffers, unmaps virtual memory
/// regions, and closes backing OS file descriptors alongside exclusive advisory file locks without
/// leaks
///
/// ## Graceful Shutdown
///
/// RAII resource cleanup must not be conflated with durability guarantees
///
/// Dropping an instance initiates a best-effort background flush, but Rust's `Drop` interface
/// discards return values, hence delayed writeback failures cannot be propagated to the caller and
/// will fail silently
///
/// Applications performing graceful shutdown must establish an explicit durability barrier before
/// dropping the instance,
///
/// * Under periodic sync configurations, await outstanding write completion via
///   [`TurboTicket::wait`] or poll [`TurboTicket::is_durable`]
/// * Under manual sync configurations, trigger a synchronous write barrier via
///   [`TurboTicket::force`] or [`TurboFox::sync`], verifying the returned [`TurboResult`]
///
/// Single-process exclusivity is enforced at startup via mandatory advisory file locks (`flock` on
/// Unix, `LockFileEx` on Windows); concurrent opening returns [`TurboError::Storage`]
///
/// ## Example
///
/// ```
/// use turbofox::{TurboCfg, TurboFox};
///
/// let dir = tempfile::tempdir().unwrap();
/// let path = dir.path();
/// let cfg = TurboCfg::manual(path);
/// let db = TurboFox::create(cfg).unwrap();
///
/// let handle = db.clone();
/// handle.set(b"shared", b"payload").unwrap();
/// assert_eq!(db.get(b"shared").unwrap(), Some(b"payload".to_vec()));
///
/// drop(handle);
/// db.delete().unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct TurboFox {
    engine: Arc<Engine>,
}

impl TurboFox {
    /// Create and initialize a new [`TurboFox`] storage layout in the target directory
    ///
    /// Formats initial container metadata (8 KiB layout across 16 KiB initial file allocation)
    /// and pre-allocates backing pages based on optional [`CapacityHint`]
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if database files already exist in the target directory or if
    ///   initial page allocation and virtual memory mapping fail
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.total_entries(), 0x00);
    /// db.delete().unwrap();
    /// ```
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Open an existing [`TurboFox`] instance from disk
    ///
    /// Internally validates file magic (`b"TBFX"`), format layout version (0x04), and host system
    /// page size alignment constraints; does not initialize missing files
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if database files do not exist or cannot be accessed
    /// * [`TurboError::CorruptedHeader`] if header validation or layout checks fail
    /// * [`TurboError::InvalidMagic`] if header signature does not match `b"TBFX"`
    /// * [`TurboError::VersionMismatch`] if layout version does not match engine version
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    ///
    /// {
    ///     let db = TurboFox::create(cfg.clone()).unwrap();
    ///     db.set(b"persisted", b"true").unwrap();
    ///     db.sync().unwrap();
    /// }
    ///
    /// let reopened = TurboFox::open(cfg).unwrap();
    /// assert_eq!(reopened.get(b"persisted").unwrap(), Some(b"true".to_vec()));
    ///
    /// reopened.delete().unwrap();
    /// ```
    pub fn open(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Open an existing [`TurboFox`] database or initialize a fresh instance if absent
    ///
    /// If backing database files already exist at the target path, opens and validates them;
    /// otherwise formats a fresh database layout
    ///
    /// Corrupt headers or invalid magic bytes are not masked or silently overwritten, returning
    /// validation errors immediately
    ///
    /// | Method           | Target Exists | Target Missing | Target Corrupt | Recommended Use Case        |
    /// |:-----------------|:--------------|:---------------|:---------------|:----------------------------|
    /// | `open_or_create` | Opens DB      | Formats DB     | Fails (`Err`)  | Application boot sequences  |
    /// | `create`         | Fails (`Err`) | Formats DB     | N/A            | Provisioning & cold setups  |
    /// | `open`           | Opens DB      | Fails (`Err`)  | Fails (`Err`)  | Strict crash restarts       |
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if files cannot be created or accessed
    /// * [`TurboError::CorruptedHeader`] if existing file header validation fails
    /// * [`TurboError::InvalidMagic`] if existing file magic signature is invalid
    /// * [`TurboError::VersionMismatch`] if layout version does not match engine version
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    ///
    /// {
    ///     let db = TurboFox::open_or_create(cfg.clone()).unwrap();
    ///     db.set(b"service", b"active").unwrap();
    ///     db.sync().unwrap();
    /// }
    ///
    /// let reopened = TurboFox::open_or_create(cfg).unwrap();
    /// assert_eq!(reopened.get(b"service").unwrap(), Some(b"active".to_vec()));
    ///
    /// reopened.delete().unwrap();
    /// ```
    pub fn open_or_create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::open_or_create(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Insert or update a key-value pair under optimistic concurrency control (OCC)
    ///
    /// Commits the entry immediately for concurrent readers and returns a [`TurboTicket`] tracking
    /// background write durability
    ///
    /// In-place overwrites execute without reallocation when updated payloads fit existing slot
    /// capacity; otherwise dynamically allocates expanded slots
    ///
    /// Key length must satisfy $0x01 \le \text{len} \le 0xFF$ bytes ([`crate::MAX_KEY_LEN`]);
    /// value length must not exceed $0xFFFF$ bytes ([`crate::MAX_VAL_LEN`])
    ///
    /// Multi-threaded write conflicts retry automatically with randomized exponential backoff up to
    /// 0x20 cycles
    ///
    /// ## Crash Safe Durability
    ///
    /// Returns a [`TurboTicket`] bound to this write's assigned commit epoch; durability barriers
    /// are epoch-cumulative, so syncing ticket $N$ persists all mutations $0..N$ simultaneously
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if physical storage expansion fails
    /// * [`TurboError::PayloadTooLarge`] if key or value exceeds maximum payload limits
    /// * [`TurboError::CollisionLimit`] if trie depth recursion reaches maximum ceiling
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// let ticket = db.set(b"key_a", b"val_a").unwrap();
    /// assert!(ticket.epoch() > 0x00);
    /// assert_eq!(db.get(b"key_a").unwrap(), Some(b"val_a".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieve value payload for the given key lock-free directly from memory-mapped pages
    ///
    /// Bypasses shared mutexes and spinlocks, routing lookups through 32-bit XxHash32 bucket chains
    /// with SIMD vector scanning
    ///
    /// Reading automatically validates CRC32 payload checksums alongside byte-level key equality;
    /// torn reads during concurrent writes fail safely via [`TurboError::CorruptedPayload`]
    ///
    /// Non-existent keys short-circuit at bucket index level without touching payload pages,
    /// returning `Ok(None)` with zero heap allocations
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`] if payload CRC32 integrity check fails
    /// * [`TurboError::InvalidType`] if the key points to a list structure
    /// * [`TurboError::Storage`] if inspecting mapped pages fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.get(b"missing").unwrap(), None);
    ///
    /// db.set(b"found", b"content").unwrap();
    /// assert_eq!(db.get(b"found").unwrap(), Some(b"content".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn get(&self, key: &[u8]) -> TurboResult<Option<Vec<u8>>> {
        self.engine.get(key)
    }

    /// Check whether the specified key exists without allocating or copying its value payload
    ///
    /// Probes key index existence directly at bucket index level; significantly faster than
    /// [`TurboFox::get`] when presence verification does not consume payload bytes
    ///
    /// ## Errors
    ///
    /// * [`TurboError::InvalidType`] if key exists as a list
    /// * [`TurboError::Storage`] if reading memory-mapped pages fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(!db.contains_key(b"probe").unwrap());
    ///
    /// db.set(b"probe", b"data").unwrap();
    /// assert!(db.contains_key(b"probe").unwrap());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn contains_key(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.contains_key(key)
    }

    /// Delete an existing key-value entry or [`TurboList`] entry in place
    ///
    /// Reclaims slots via tombstones directly without global table redistribution or stop-the-world
    /// compaction pauses
    ///
    /// Returns `Ok(true)` if entry was found and removed; `Ok(false)` if absent
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if metadata writeback fails during deletion
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// db.set(b"temp_key", b"temp_val").unwrap();
    /// assert!(db.del(b"temp_key").unwrap());
    /// assert!(!db.del(b"temp_key").unwrap());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn del(&self, key: &[u8]) -> TurboResult<bool> {
        self.engine.del(key)
    }

    /// Access or create an ordered 1-to-N [`TurboList`] sequence anchored at the given key
    ///
    /// Sequence elements are co-located in adjacent array descriptor blocks inside the primary
    /// storage container file without auxiliary tables or relational join overhead
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// let list = db.list(b"sensor_log");
    /// list.append(b"reading_0").unwrap();
    /// list.append(b"reading_1").unwrap();
    ///
    /// assert_eq!(list.len().unwrap(), 0x02);
    /// assert_eq!(list.first().unwrap(), Some(b"reading_0".to_vec()));
    /// assert_eq!(list.last().unwrap(), Some(b"reading_1".to_vec()));
    ///
    /// drop(list);
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn list(&self, key: &[u8]) -> TurboList {
        TurboList::new(Arc::clone(&self.engine), key.to_vec())
    }

    /// Return the total count of active key-value and [`TurboList`] entries present in storage
    ///
    /// Tracked in memory via `AtomicU64`, avoiding writer cache-line contention on Metadata Slot 0
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert_eq!(db.total_entries(), 0x00);
    /// db.set(b"k1", b"v1").unwrap();
    /// db.set(b"k2", b"v2").unwrap();
    /// assert_eq!(db.total_entries(), 0x02);
    ///
    /// db.del(b"k1").unwrap();
    /// assert_eq!(db.total_entries(), 0x01);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn total_entries(&self) -> u64 {
        self.engine.total_entries()
    }

    /// Return `true` if the database contains zero active key-value or [`TurboList`] entries
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.is_empty());
    /// db.set(b"k", b"v").unwrap();
    /// assert!(!db.is_empty());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total_entries() == 0
    }

    /// Return the total number of bytes currently resident in physical host memory (RAM)
    ///
    /// Combines file-backed pages currently resident in RAM queried via host page tables
    /// (`mincore` on POSIX / `QueryWorkingSetEx` on Windows) with heap memory allocated by
    /// in-memory resolution caches
    ///
    /// ## Memory Semantics
    ///
    /// Unlike private heap allocations (such as `Vec<u8>` or `String`) which represent committed
    /// anonymous RSS memory that cannot be dropped without swap or OOM termination, file-backed
    /// memory represents pages held in the operating system unified page cache
    ///
    /// The host kernel retains autonomous control over this memory; clean mapped pages can be
    /// reclaimed, trimmed, or evicted at any moment under system memory pressure without
    /// process involvement or data loss
    ///
    /// Consequently, this metric is a transient point-in-time snapshot of pages resident in
    /// physical RAM rather than a fixed process allocation or memory commitment
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if inspecting host virtual memory page residency fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.resident_bytes_in_memory().unwrap() > 0x00);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn resident_bytes_in_memory(&self) -> TurboResult<usize> {
        self.engine.resident_bytes_in_memory()
    }

    /// Evict database pages from physical memory (RAM) according to the configured mode
    ///
    /// ## Semantics
    ///
    /// Synchronizes dirty metadata and data pages to persistent storage via [`sync`](Self::sync)
    /// prior to eviction to guarantee persistence and advance durable write epochs, marking
    /// all outstanding [`TurboTicket`] instances durable
    ///
    /// Eviction operates as a best-effort kernel advisory hint; while process page table entries
    /// (PTEs) or working set pages are unmapped, underlying clean file pages may remain resident
    /// in the operating system page cache according to host kernel memory management policies
    ///
    /// In [`EvictionMode::PreserveHeaders`], advises the kernel to unmap data and index pages
    /// while retaining metadata and root node pages hot in cache
    ///
    /// In [`EvictionMode::PurgeAll`], advises the kernel to unmap all mapped pages across the
    /// entire database file; subsequent operations demand-page faulted blocks back into RAM
    ///
    /// When lookup caching is enabled, it invalidates allocated heap cache chunks
    ///
    /// Subsequent calls to [`TurboFox::sync`] while pages remain cold or evicted succeed
    /// as a zero-cost no-op without faulting pages back into physical memory
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if underlying storage or I/O failures occur during pre-eviction sync
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{EvictionMode, TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let cfg = TurboCfg::manual(dir.path());
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// db.set(b"k1", b"v1").unwrap();
    /// assert!(db.resident_bytes_in_memory().unwrap() > 0x00);
    ///
    /// db.evict(EvictionMode::PreserveHeaders).unwrap();
    /// assert!(db.resident_bytes_in_memory().is_ok());
    /// assert_eq!(db.get(b"k1").unwrap(), Some(b"v1".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn evict(&self, mode: EvictionMode) -> TurboResult<()> {
        self.engine.evict(mode)
    }

    /// Return the active file length in bytes allocated on persistent disk storage
    ///
    /// Starts at 16 KiB (`0x4000` bytes) for a cold database layout and expands dynamically in
    /// uniform OS page increments (16 KiB / 64 KiB) under `growth_mutex` coordination
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.resident_bytes_on_disk() >= 0x4000);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn resident_bytes_on_disk(&self) -> usize {
        self.engine.resident_bytes_on_disk()
    }

    /// Perform an explicit flush barrier for crash safe durability
    ///
    /// Forces dirty pages to physical NVMe storage (`msync` on POSIX / `FlushViewOfFile` on Windows),
    /// advances the durable write epoch monotonically, and notifies outstanding ticket listeners
    ///
    /// If no modifications occurred or if all pages remain cold/evicted, returns immediately as a
    /// safe, zero-cost no-op without faulting cold pages into memory
    ///
    /// Durability barriers are epoch-cumulative, simultaneously guaranteeing persistence of all
    /// writes up to the current commit epoch
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if physical synchronization or metadata writeback fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// db.set(b"key", b"value").unwrap();
    /// assert!(db.sync().is_ok());
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn sync(&self) -> TurboResult<()> {
        self.engine.sync()
    }

    /// Close the database, unmap virtual memory, and delete backing container files from disk
    ///
    /// Requires that all clones, child [`TurboList`], and [`TurboListIter`] handles
    /// have been dropped (`Arc::try_unwrap`)
    ///
    /// ## Errors
    ///
    /// * [`TurboError::ResourceBusy`] if other active references or handles to this instance exist
    /// * [`TurboError::Storage`] if filesystem file removal fails
    ///
    /// ## Example
    ///
    /// ```
    /// use turbofox::{TurboCfg, TurboFox};
    ///
    /// let dir = tempfile::tempdir().unwrap();
    /// let path = dir.path();
    /// let cfg = TurboCfg::manual(path);
    /// let db = TurboFox::create(cfg).unwrap();
    ///
    /// assert!(db.delete().is_ok());
    /// ```
    pub fn delete(self) -> TurboResult<()> {
        match Arc::try_unwrap(self.engine) {
            Ok(engine) => engine.delete(),
            Err(_) => Err(TurboError::ResourceBusy(
                "cannot delete database with active concurrent handles",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ok_turbofox_lifecycle_and_crud() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let cfg = TurboCfg::manual(&path);

        let db = TurboFox::open_or_create(cfg).unwrap();
        assert!(db.is_empty());
        assert_eq!(db.total_entries(), 0);
        assert!(!db.contains_key(b"key1").unwrap());
        assert!(!db.del(b"key1").unwrap());

        let ticket = db.set(b"key1", b"val1").unwrap();
        assert!(ticket.epoch() > 0);
        assert_eq!(db.get(b"key1").unwrap().as_deref(), Some(&b"val1"[..]));
        assert!(db.contains_key(b"key1").unwrap());
        assert_eq!(db.total_entries(), 1);
        assert!(!db.is_empty());

        assert!(db.resident_bytes_in_memory().unwrap() > 0);
        assert!(db.resident_bytes_on_disk() > 0);

        db.sync().unwrap();
        db.evict(EvictionMode::PreserveHeaders).unwrap();
        assert_eq!(db.get(b"key1").unwrap().as_deref(), Some(&b"val1"[..]));

        assert!(db.del(b"key1").unwrap());
        assert_eq!(db.total_entries(), 0);
        assert!(db.is_empty());
        assert!(!db.contains_key(b"key1").unwrap());

        db.sync().unwrap();
        drop(db);

        let reopen_cfg = TurboCfg::manual(&path);
        let reopened = TurboFox::open(reopen_cfg).unwrap();
        assert!(reopened.is_empty());
        assert_eq!(reopened.total_entries(), 0);

        reopened.delete().unwrap();
    }

    #[test]
    fn err_turbofox_delete_busy_concurrent_handle() {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path());
        let db = TurboFox::create(cfg).unwrap();

        let list = db.list(b"concurrent_list");
        let delete_res = db.delete();
        assert!(matches!(delete_res, Err(TurboError::ResourceBusy(_))));

        drop(list);
    }
}
