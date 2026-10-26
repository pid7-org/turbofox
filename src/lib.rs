//! An embedded, memory-mapped key-value engine providing lock-free concurrency and explicit sync
//! barriers
//!
//! ## Key-Value Storage
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let cfg = TurboCfg::manual(path);
//! let db = TurboFox::create(cfg).unwrap();
//!
//! let ticket = db.set(b"device:01", b"state:active").unwrap();
//! assert_eq!(db.get(b"device:01").unwrap(), Some(b"state:active".to_vec()));
//!
//! db.sync().unwrap();
//! assert!(ticket.is_durable());
//!
//! assert!(db.del(b"device:01").unwrap());
//! assert_eq!(db.get(b"device:01").unwrap(), None);
//!
//! db.delete().unwrap();
//! ```
//!
//! ## List Operations
//!
//! ```
//! use turbofox::{TurboCfg, TurboFox};
//!
//! let dir = tempfile::tempdir().unwrap();
//! let path = dir.path();
//! let cfg = TurboCfg::manual(path);
//! let db = TurboFox::create(cfg).unwrap();
//!
//! let list = db.list(b"events");
//! list.append(b"boot").unwrap();
//! list.append(b"init").unwrap();
//!
//! assert_eq!(list.len().unwrap(), 2);
//! assert_eq!(list.pop().unwrap(), Some(b"init".to_vec()));
//! assert_eq!(list.pop().unwrap(), Some(b"boot".to_vec()));
//! assert_eq!(list.pop().unwrap(), None);
//!
//! drop(list);
//! db.delete().unwrap();
//! ```
//!
//! ## Architecture Guarantees
//!
//! TurboFox is an embedded key-value storage engine engineered around memory-mapped slot
//! topography, optimistic concurrency control, and zero-rehash overflow chaining
//!
//! ### Zero Rehash
//!
//! Primary key routing utilizes 64-bit Wyhash bit-sliced into depth-indexed buckets (64 slots each)
//!
//! Hash collisions resolve via a localized 4-slot probing window followed by overflow bucket
//! chaining,
//!
//! * Entries are permanently anchored to their allocated bucket chain and never rehashed across
//!   files, which eliminates stop-the-world hash table redistribution pauses while sustaining
//!   bounded p99 write latency
//! * In-place overwrites execute directly within preallocated DataBuf spans when new payloads fit
//!   in existing 16-byte chunk boundaries
//! * Structural allocations expand incrementally via monotonic bump allocation w/o bulk page copies
//!
//! ### Torn-Write Protection
//!
//! Point reads execute lock-free w/o reader-writer locks or shared mutex acquisition
//!
//! Readers probe 64-bit hash sentinels directly within mapped virtual memory cache lines,
//!
//! * Payloads utilize binary framing: `[ CRC32 (4 B) ][ Key ][ Val ][ Pad ]`
//! * Reading validates CRC32 integrity alongside byte-level key equality (concurrent writes or
//!   partial page flushes that cause torn reads fail safely via `TurboError::CorruptedPayload`
//!   rather than returning corrupt data)
//! * Negative queries (`Ok(None)`) short-circuit on zero-sentinels w/o accessing payload pages;
//!   zero heap allocations occur during probe traversal and miss evaluations
//!
//! ### Durability Model
//!
//! Mutations (`set`, `del`, `append`, `pop`) commit into memory-mapped slots immediately via OCC
//! and return a lightweight `TurboTicket` bound to a monotonic commit epoch,
//!
//! * Decouples memory-speed writes from physical NVMe writeback latency, where durability
//!   guarantees depend on the configured sync strategy
//! * Manual Mode (`sync_interval: None`),
//!   * The engine performs no automatic background flushing
//!   * The caller must invoke `db.sync()` or block on `ticket.wait()` to guarantee crash safety on
//!     disk
//!   * Un-flushed mutations remain volatile against OS kernel panics or host power loss
//! * Periodic Sync Mode (`sync_interval: Some(dur)`),
//!   * A dedicated background sync thread flushes dirty pages up to the current epoch
//!   * Incorporates adaptive backoff under idle load to prevent write endurance wear on flash
//!     devices
//! * `ticket.is_durable()` enables non-blocking checks to poll writeback completion asynchronously
//!
//! ### TurboList
//!
//! Ordered 1-to-N sequences co-exist inside the primary storage file
//!
//! Anchored by a primary key registered in root buckets w/ type flag
//! `offsets[i][0] & 0x80 != 0`,
//!
//! * Eliminates secondary index tables, auxiliary catalog files, and relational join overhead
//! * Sequences chain through compact 20-byte `AdjArrayList` descriptor blocks (4 item slots per
//!   node)
//! * Tail appends sustain amortized O(1) latency (pops deallocate item DataBuf spans and return
//!   empty descriptor blocks to allocation bitmaps)
//! * Readers traverse chained descriptors w/ index-based skipping (`idx / 0x04`) and CRC32 checks
//!
//! ### Storage Topography
//!
//! Unified storage container structured into uniform 128-byte slots
//!
//! Initial file footprint begins at only 16 KiB (0x80 slots) on standard 4 KiB host page systems,
//!
//! * Virtual memory space reserves up to `MAX_DB_LEN` upfront (physical disk pages back lazily as
//!   monotonic bump-pointer slot markers advance)
//! * Buffer pools track 16-byte chunks via 64-bit bitmaps, eliminating internal space fragmentation
//! * Backing files grow dynamically in uniform OS page increments, minimizing write amplification
//!
//! ### Operational Invariants
//!
//! Hardware and protocol constraints callers must design for,
//!
//! * Key length bounded between 0x01 and 0xFF bytes and value payload size capped at 0xFFFF bytes
//! * Contiguous buffer spans within standard DataBuf blocks cap at 0x40 buffers (1024 bytes) while
//!   larger payloads span chained segment buffers
//! * Multi-slot writes commit via lock-free OCC w/ conflict detection across mapped memory slots
//!   (aborts and retries up to 0x20 times before surfacing contention errors)
//! * Thread-safe (`Send + Sync`) within a single process via `Arc<TurboFox>`
//!   * Single-process exclusivity is enforced at startup via mandatory POSIX / Win32 advisory file
//!     locks
//!   * Concurrent access by multiple OS processes on the same file is strictly prohibited
//!
//! ## Benchmarks
//!
//! ### Evaluation Environment
//!
//! Benchmarks execute on bare-metal virtualized cloud instances w/ direct NVMe storage:
//!
//! - Instance: AWS EC2 `c5d.large` (2 vCPUs, Intel Xeon Platinum 8223CL CPU @ 3.00 GHz)
//! - Memory: 0x04 GiB DDR4 system RAM
//! - Storage: Dedicated local hardware NVMe SSD (`/mnt/nvme0n1`)
//! - Workload: 0x186A0 (100,000) operations per benchmark suite under uniform distribution
//! - Concurrency: 1-thread baseline vs 4-thread concurrent worker evaluation
//!
//! ### Operation Throughput & Concurrency Scaling
//!
//! Sustained key-value operations per second across 1-thread and 4-thread worker configurations
//!
//! | Workload              | 1 Thread Throughput | 4 Thread Throughput | Scaling Ratio |
//! |:----------------------|:--------------------|:--------------------|:--------------|
//! | SET (Insert)          | 1.18 kops/s         | 963 ops/s           | 0.82x         |
//! | GET (60/40 Hit/Miss)  | 1.95 kops/s         | 1.78 kops/s         | 0.91x         |
//! | DEL (60/40 Hit/Miss)  | 1.95 kops/s         | 1.78 kops/s         | 0.91x         |
//!
//! Note: Evaluation executes on 2 vCPU hardware where 4 worker threads oversubscribe physical
//! compute cores; throughput scaling reflects compute core oversubscription and OCC slot contention
//!
//! ### Tail Latency Profile Under Concurrency
//!
//! Percentile latency distributions (p50 to p99.9) comparing 1T baseline against 4T concurrent
//! workers
//!
//! | Workload              | Threads | p50     | p90     | p99     | p99.9     |
//! |:----------------------|:--------|:--------|:--------|:--------|:----------|
//! | SET: Insert           | 1T      | 0.81 ms | 1.42 ms | 3.11 ms | 3.99 ms   |
//! | SET: Insert           | 4T      | 0.96 ms | 1.94 ms | 4.57 ms | 422.58 ms |
//! | GET: 60/40 Hit/Miss   | 1T      | 0.28 ms | 1.42 ms | 1.68 ms | 1.71 ms   |
//! | GET: 60/40 Hit/Miss   | 4T      | 0.62 ms | 4.69 ms | 8.04 ms | 10.45 ms  |
//! | DEL: 60/40 Hit/Miss   | 1T      | 0.29 ms | 1.42 ms | 1.68 ms | 1.71 ms   |
//! | DEL: 60/40 Hit/Miss   | 4T      | 0.61 ms | 4.61 ms | 7.60 ms | 10.35 ms  |
//!
//! Note: Sub-millisecond median latencies (p50 < 0.30 ms for reads and deletes) sustained under 1T
//! baseline; read and delete tail latencies stay bounded under 10.5 ms at p99.9 under 4T
//! concurrency while elevated 4T p99.9 write tail stems from OCC conflict retries and backing
//! file growth serialization

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
mod ticket;

pub use cfg::{SyncMode, TurboCfg};
pub use common::{MAX_KEY_LEN, MAX_VAL_LEN};
pub use error::{TurboError, TurboResult};
pub use list::{TurboList, TurboListIter};
pub use ticket::TurboTicket;

use engine::Engine;
use std::sync::Arc;

/// An embedded, memory-mapped key-value engine providing lock-free concurrency and explicit sync
/// barriers
///
/// Internally wraps an `Arc<Engine>`; cloning is cheap and shares the underlying memory mapping,
/// hash table, and sync worker thread pool across threads
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
    /// Create and initialize a new [`TurboFox`] instance in the target directory
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if database files already exist in the target directory or if
    ///   initial page allocation and mapping fail
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
    /// assert_eq!(db.total_entries(), 0);
    /// db.delete().unwrap();
    /// ```
    pub fn create(cfg: TurboCfg) -> TurboResult<Self> {
        let engine = Engine::create(cfg)?;

        Ok(Self { engine: Arc::new(engine) })
    }

    /// Open an existing [`TurboFox`] instance from disk
    ///
    /// Internally verifies header magic, layout version, and host system page size constraints
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if the database files do not exist or cannot be accessed
    /// * [`TurboError::CorruptedHeader`] if header validation or layout checks fail
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

    /// Insert or update a key-value pair
    ///
    /// Internally commits bytes into memory-mapped data buffers and returns a [`TurboTicket`]
    /// tracking background write durability
    ///
    /// Key length must not exceed [`crate::MAX_KEY_LEN`] bytes and value payload must not exceed
    /// [`crate::MAX_VAL_LEN`] bytes
    ///
    /// ## Crash Safe Durability
    ///
    /// Returns a [`TurboTicket`] bound to this write's assigned commit epoch
    ///
    /// Internally write is commited to memory-mapped buffers immediately for lock-free read
    /// availability, while the ticket allows callers to track when bytes are physically flushed for
    /// crash safe durability
    ///
    /// Callers requiring durability guarantees can call [`TurboTicket::wait`] to block until
    /// persisted, or [`TurboTicket::force`] to trigger an immediate flush
    ///
    /// Fire-and-forget callers can safely discard the ticket without blocking
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if physical storage expansion fails
    /// * [`TurboError::PayloadTooLarge`] if key or value exceeds maximum payload limits
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
    /// assert!(ticket.epoch() > 0);
    /// assert_eq!(db.get(b"key_a").unwrap(), Some(b"val_a".to_vec()));
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline(always)]
    pub fn set(&self, key: &[u8], val: &[u8]) -> TurboResult<TurboTicket> {
        self.engine.set(key, val)
    }

    /// Retrieve value payload for the given key
    ///
    /// Internally copies bytes from memory-mapped storage into an owned `Vec<u8>` after verifying
    /// payload CRC32 integrity
    ///
    /// Returns `Ok(None)` if key does not exist or has been deleted
    ///
    /// ## Errors
    ///
    /// * [`TurboError::CorruptedPayload`] if payload CRC32 integrity check fails
    /// * [`TurboError::InvalidType`] if the key points to a list structure
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
    /// Internally performs lock-free bucket probing directly in mapped hash tables; significantly
    /// faster than [`TurboFox::get`] when checking for existence
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

    /// Delete an existing key-value entry or [`TurboList`] entry from the [`TurboFox`]
    ///
    /// Returns `Ok(true)` if entry was found and removed, `Ok(false)` if absent
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

    /// Accesse or create a 1-to-N [`TurboList`] structure anchored at the given key
    ///
    /// It allows sequential appends, pops, and index-based random access
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
    /// assert_eq!(list.len().unwrap(), 2);
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

    /// Return the total count of active key-value and [`TurboList`] entries present in the
    /// [`TurboFox`]
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
    /// assert_eq!(db.total_entries(), 0);
    /// db.set(b"k1", b"v1").unwrap();
    /// db.set(b"k2", b"v2").unwrap();
    /// assert_eq!(db.total_entries(), 2);
    ///
    /// db.del(b"k1").unwrap();
    /// assert_eq!(db.total_entries(), 1);
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

    /// Return the total count of physical memory-mapped pages currently allocated to the database
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if reading metadata fails
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
    /// assert!(db.total_pages().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn total_pages(&self) -> TurboResult<u32> {
        self.engine.total_pages()
    }

    /// Return the remaining physical pages available before hitting maximum database capacity
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if querying database mapping length fails
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
    /// assert!(db.remaining_pages().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn remaining_pages(&self) -> TurboResult<u32> {
        self.engine.remaining_pages()
    }

    /// Return the remaining available storage capacity in bytes before reaching capacity limit
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if querying database mapping length fails
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
    /// assert!(db.remaining_bytes().unwrap() > 0);
    ///
    /// db.delete().unwrap();
    /// ```
    #[inline]
    pub fn remaining_bytes(&self) -> TurboResult<u64> {
        self.engine.remaining_bytes()
    }

    /// Perform an explicit flush for crash safe durability
    ///
    /// ## Errors
    ///
    /// * [`TurboError::Storage`] if synchronization or metadata writeback fails
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

    /// Close the database and delete all backing files from disk
    ///
    /// The call requires that all clones and child [`TurboList`] handles have been dropped
    ///
    /// ## Errors
    ///
    /// * [`TurboError::ResourceBusy`] if other active references to this instance exist
    /// * [`TurboError::Storage`] if filesystem file deletion fails
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
