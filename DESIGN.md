# Technical Specs

TurboFox uses a single file storage layout, embedded on users device to manage key-value and list
entries

## 1. Storage Hierarchy & Invariants

```
Unit                 Size      Slots      Alignment  Purpose
-------------------+---------+----------+----------+--------------------------------
Slot (T)           64 B      1          8 B        Base IO / spinlock concurrency
AdjArrayBlock      128 B     2          8 B        1-to-N list index block
Metadata           64 B      1          8 B        Engine configuration, heads, and spare nodes
Node               64 B      1          8 B        Internal 15-way trie routing node
Bucket             256 B     4          8 B        32-entry leaf bucket
DBuf               256 B     4          8 B        Contiguous payload buffer pool
Physical Page 4K   4096 B    64         4096 B     Host filesystem growth quantum
Physical Page 16K  16384 B   256        16384 B    Host filesystem growth quantum
Physical Page 64K  65536 B   1024       65536 B    Host filesystem growth quantum
```

The following invariants must hold,

- All on-disk structures use `#[repr(C)]`, occupy an integer number of `Slot` units (64 B), and are
  aligned to 8 bytes.
- File growth is quantized to physical OS page sizes of 4 KiB, 16 KiB, or 64 KiB.
- A 64 B Slot is the atomic transaction boundary. Concurrency is managed with Optimistic Concurrency
  Control (OCC). Transactions acquire and validate multiple slot locks in order to avoid deadlocks.
- There is no virtual page layer. Every component address resolves directly to an absolute slot
  index.
- Allocation within the mapped file uses a monotonic bump pointer. Allocations are rounded to 256 B
  (4 slots) to eliminate fragmentation. In-place updates are limited to atomic Bucket-to-Node split
  publication and OCC slot writes.

## 2. File Topography (Initial Footprint)

The initial structure occupies exactly 8192 B (0x80 slots / 128 slots). The backing file maps 16 KiB
(0x0100 slots) on 4 KiB and 16 KiB systems, and 64 KiB (0x0400 slots) on 64 KiB systems.

```
Slot Range   Byte Range     Size     Component
-----------+--------------+--------+--------------------------------------------------
0            0x0000..0x003F 64 B     Slot 0: Metadata
1            0x0040..0x007F 64 B     Slot 1: Root Node (ROOT_NODE_SLOT)
2..3         0x0080..0x00FF 128 B    Slots 2..3: AdjArrayBlock head (ADJ_ARR_HEAD_SLOT)
4..63        0x0100..0x0FFF 3840 B   15x Initial Buckets (child indices 0x00..0x0E, 256 B each)
64..123      0x1000..0x1EFF 3840 B   15x DBuf pools (DATABUF_HEAD_SLOT, 256 B each)
124..127     0x1F00..0x1FFF 256 B    4x Pre-allocated spare Nodes (64 B each)
128..N       0x2000..EOF    Rest     Unallocated slots managed by free_slot_marker
```

- *Initial index:* One Root Node at Slot 1 (`ROOT_NODE_SLOT = 0x01`), routing to 15 Buckets.
- *Initial payload storage:* 15 DBuf pools, providing 465 buffers of 8 B each
  (head `DATABUF_HEAD_SLOT = 0x40`).
- *Initial list storage:* One AdjArray block containing 6 list arrays
  (head `ADJ_ARR_HEAD_SLOT = 0x02`).
- *Initial spare nodes:* Four pre-allocated Nodes at Slots 124..127, tracked in Metadata.
- *Initially unallocated space:* Slots 0x80..0xFF in a 16 KiB mapping, or Slots 0x80..0x03FF in a 64
  KiB mapping. `free_slot_marker` starts at 128 (`0x80`), aligned to a 256 B boundary.

## 3. Component Memory Layouts

### 3.1 Static Topography Constants & Metadata (Slot 0, 64 Bytes, `align(8)`)

#### Engine Static Head Constants

The head slot indices for base components remain fixed for the lifetime of the database. They are
defined as immutable compile-time constants,

```rs
pub const ROOT_NODE_SLOT: u32 = 0x01;    // Slot 1  (Byte offset 0x0040)
pub const ADJ_ARR_HEAD_SLOT: u32 = 0x02; // Slot 2  (Byte offset 0x0080)
pub const DATABUF_HEAD_SLOT: u32 = 0x40; // Slot 64 (Byte offset 0x1000)
pub const MAX_TRIE_DEPTH: usize = 0x08;  // Depth ceiling guarding hash collisions
```

#### Metadata Layout

```
Offset   Type      Field             Description
-------+---------+-----------------+--------------------------------------------------
0..3     [u8; 4]   magic             0x54424658 ("TBFX")
4..7     u32       version           Layout version (3)
8..11    u32       page_size         Host OS page size (4096, 16384, or 65536)
12..15   u32       total_pages       Total allocated physical OS pages
16..19   u32       total_slots       Total addressable slots in mapped file
20..23   u32       free_slot_marker  Next unallocated slot index (starts at 128 / 0x80)
24..31   u64       total_entries     Active key count (KV + List)
32..47   [u32; 4]  extra_nodes       Pre-allocated spare Node slot stack
48..51   u32       extra_node_count  Active spare Node count in stack (starts at 4)
52..63   [u8; 12]  reserved          Zero-padded reserved space
```

### 3.2 Node (64 Bytes, 1 Slot, `align(8)`)

A Node is an internal trie routing structure with up to 15 child Buckets or lower-level Nodes; it
occupies exactly one slot (64 B):

```
Offset   Type        Field           Description
-------+-----------+---------------+--------------------------------------------------
0..3     u32         flag            Child component type bitmap (0 = Bucket, 1 = Node)
4..63    [u32; 15]   bucket_indices  Slot indices of 15 child Buckets or Nodes
```

- `flag`: Bits 0..14 form a bitmask indicating child component type for each index $i \in [0, 14]$
  (`0` = leaf Bucket, `1` = child Node); bits 15..31 are reserved (`0x0000`)
- `bucket_indices[i]`: Absolute slot index of the child component associated with slice
  $i \in [0, 14]$

### 3.3 Bucket (256 Bytes, 4 Slots, `align(8)`)

A Bucket is a leaf-level hash table holding up to 32 entries; it occupies exactly four slots
(256 B) with zero per-bucket header overhead:

```
Offset    Type        Field    Description
--------+-----------+--------+--------------------------------------------------------
0..127    [u32; 32]   hashes   Normalized key hashes (0 = empty, 1 = tomb, >= 2 valid)
128..255  [u32; 32]   offsets  Global DBuf buffer indices (global_buf_idx)
```

#### Hash Encoding & Sentinels (`hashes[i]`)

- `0x0000_0000`: Empty slot sentinel (`HASH_EMPTY`)
- `0x0000_0001`: Tombstone (deleted) sentinel (`HASH_TOMBSTONE`)
- Bit 31 Discriminator:
  - Bit 31 = `0`: *KV Entry* (`hash & 0x7FFF_FFFF`, normalized $\ge 2$)
  - Bit 31 = `1`: *List Entry* (`hash | 0x8000_0000`, normalized $\ge 2^{31}$)
- KV and List keyspaces are completely disjoint; identical keys do not collide

#### Entry Buffer Index (`offsets[i]`, 4 Bytes)

Each offset stores a `u32` global buffer index (`global_buf_idx`) pointing to the entry framing
head in DBuf; length metadata and list pointer array heads are stored in the payload buffer header

### 3.4 DBuf (256 Bytes, 4 Slots, `align(8)`)

A DBuf is a contiguous pool of 31 payload buffers, each 8 bytes. It occupies exactly four slots
(256 B),

```
Offset    Type            Field      Description
--------+---------------+----------+--------------------------------------------------
0..247    [[u8; 8]; 31]   buffers    31 payload buffers (8 B each = 248 B)
248..251  u32             next_dbuf  Slot index of next DBuf in chain (0 = tail)
252..255  u32             bmap       Bits 0..30: buffer occupancy; Bit 31: pool full flag
```

- `buffers`: 31 independent 8-byte buffers for key and value payload data.
- `next_dbuf`: Absolute slot index of the next DBuf pool in the monotonic allocation chain.
- `bmap`: Each bit $i$ ($0 \le i \le 30$) tracks whether `buffers[i]` is occupied. The most
  significant bit (bit 31, mask `0x8000_0000`) is set when all 31 buffers are occupied, allowing the
  full-pool check to use a single-cycle branch (`(bmap as i32) < 0`).

#### Payload Framing (8-Byte Alignment)

Payloads are stored in contiguous buffers; buffer 0 stores the 8 B descriptor header:

- *KV Framing*:
  Allocates $1 + \lceil (\text{klen} + \text{vlen}) / 8 \rceil$ contiguous buffers
  ```
  Buffer 0 (Header, 8 B):
  [ CRC32 (4 B, LE) ][ klen (1 B) ][ vlen (3 B, LE) ]

  Buffers 1..N (Payload):
  [ Key Bytes (klen) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```
  `vlen` supports up to 16,777,215 B (`0x00FF_FFFF`), fully covering `MAX_VAL_LEN = 0xFFFF`;
  CRC32 is computed over `[klen (1 B), vlen (3 B), Key, Value]`

- *List Key Framing*:
  Allocates $1 + \lceil \text{klen} / 8 \rceil$ contiguous buffers
  ```
  Buffer 0 (Header, 8 B):
  [ CRC32 (4 B, LE) ][ klen (1 B) ][ adjarr_idx (3 B, LE) ]

  Buffers 1..N (Payload):
  [ Key Bytes (klen) ][ 0-Padding ]
  ```
  `adjarr_idx` supports up to 16,777,215 relative array indices; CRC32 is computed over
  `[klen (1 B), adjarr_idx (3 B), Key]`

- *List Item Value Framing*:
  Allocates $\lceil (6 + \text{vlen}) / 8 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ vlen (2 B, LE) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```
  Items with $\text{vlen} > 0xF2$ (242 B, up to `MAX_VAL_LEN = 0xFFFF`) span consecutive pools in
  the allocation chain; payload data skips the trailing 8-byte metadata (`next_dbuf` and `bmap`) at
  each pool boundary; these writes are committed atomically through multi-slot OCC transactions

#### Fast 2-Buffer Read Process (KV and List Key)

1. Read buffer 0 (Header, 8 B) and buffer 1 (Key prefix up to 8 B)
2. Inspect `klen` (Byte 4); if `klen != key.len()`, hash collision is rejected immediately
3. If `key.len() <= 8`, key equality is verified directly against buffer 1
4. If `local_idx < 30`, buffers 0 and 1 are contiguous in the local DBuf; if `local_idx == 30`,
   buffer 1 resides at local buffer 0 of `next_dbuf` (boundary-safe multi-pool read)
5. If key matches and `key.len() > 8`, load spanned buffers via `DBufSlotCache` to verify full key
6. For KV: extract `vlen` from bytes 5..7, stream remaining value bytes, and verify CRC32
7. For List: extract `adjarr_idx` from bytes 5..7 to access the `AdjArrayList` head

#### List Item Value Read Process

1. Read single 8 B buffer at `dbuf_idx`; contains `[ CRC32 (4 B) ][ vlen (2 B) ][ Val[0..1] ]`
2. Inspect `vlen` (Bytes 4..5, LE); if `vlen <= 2`, value resides entirely within initial buffer
3. If `vlen > 2`, compute total required buffers $N = \lceil (6 + \text{vlen}) / 8 \rceil$;
   if allocation fits within pool, read slice from single DBuf; otherwise load spanned pools
   via `DBufSlotCache` and stream bytes across pool boundaries
4. Verify CRC32 over `[vlen (2 B), Val]`

A global buffer index $B$ is translated as follows:
$$\text{pool\\_hop} = \lfloor B / 31 \rfloor$$
$$\text{local\\_buf} = B \pmod{31}$$

#### In-Memory DBuf Slot Cache (Wait-Free $O(1)$ Hop Resolution)

Resolving a physical slot by traversing the persistent `next_dbuf` chain requires one sequential
disk or memory load per pool hop, resulting in O(n) lookup time. The in-memory append-only chunked
array reduces this to wait-free O(1) lookup.

```rs
const CHUNK_SHIFT: usize = 0x0C; // 4096 entries per chunk (16 KiB chunk)
const CHUNK_SIZE: usize = 1 << CHUNK_SHIFT;
const CHUNK_MASK: usize = CHUNK_SIZE - 1;
const MAX_CHUNKS: usize = 0x0400; // 1024 chunks covers 4,194,304 DBuf pools (1 GiB DB)

pub(crate) struct DBufSlotCache {
    chunks: [AtomicPtr<[AtomicU32; CHUNK_SIZE]>; MAX_CHUNKS],
}
```

- *Read path:* The chunk index and offset within the chunk are calculated with bit shifts:
  `chunk_idx = (hop as usize) >> CHUNK_SHIFT;`
  `offset = (hop as usize) & CHUNK_MASK;`
  The chunk pointer is loaded with `Ordering::Acquire`, and the slot is read without locks or
  reference counts. The wait-free path avoids cache-line bouncing between concurrent transactions.
  Pool 0 uses a constant fast path to resolve directly to `DATABUF_HEAD_SLOT`.
- *Write path (growth):* Under `growth_mutex`, crossing a chunk boundary allocates a 16 KiB boxed
  chunk. Its pointer is published with `Ordering::Release`, and slot indices are written using
  monotonic stores.
- *Memory footprint:* The outer pointer array uses 8 KiB (`1024 * 8 B`). A 16 KiB chunk is allocated
  lazily for every 4096 DBuf pools. The cache uses 4 B per DBuf pool (1.56% of raw DBuf storage).
  The pool 0 fast path avoids allocating chunk 0 during initialization.

### 3.5 Adjacent Array Block (128 Bytes, 2 Slots, `align(8)`)

An Adjacent Array Block stores the index structure for 1-to-N list pointer chains. Each block
occupies exactly two slots (128 B).

```
Offset    Type                 Field            Description
--------+--------------------+----------------+---------------------------------------
0..119    [AdjArrayList; 6]    arrays           6 list arrays (20 B each = 120 B)
120..123  u32                  next_adjarr_idx  Slot index of next AdjArr block in chain
124..125  u16                  bmap             Bits 0..5: array occupancy mask (0x3F)
126..127  [u8; 2]              padding          Zero alignment padding to 128 B
```

#### `AdjArrayEntry` (4 Bytes, `repr(C)`)

- `dbuf_idx: u32` (4 B): Global DBuf buffer index containing `[CRC32][vlen][Value]`.

#### `AdjArrayList` (20 Bytes, `repr(C)`)

- `entries: [AdjArrayEntry; 4]` (16 B): Four list item pointers.
- `next_arr_idx: u32` (4 B): Global relative index of the next array in the chain (`0` indicates the
  tail).

#### Global Relative Array Indexing

For relative array index $A$:
$$\text{block\\_hop} = \lfloor A / 6 \rfloor$$
$$\text{local\\_array} = A \pmod{6}$$

Follow the `next_adjarr_idx` block chain for `block_hop` steps, then access `arrays[local_array]`.

## 4. Hash Routing, Trie Traversal & Splitting

### 4.1 Key Hash Normalization

Keys are hashed with `twox_hash::XxHash32`, initialized with seed `0x00` (the default `SEED` is the
fastest option); bit 31 partitions KV and List keyspaces:

```rs
let raw = hasher.finish() as u32;

// KV Hash: bit 31 = 0; normalized to >= 2 to preserve sentinels
let kv_hash = {
    let h = raw & 0x7FFF_FFFF;
    if h < 2 { h + 2 } else { h }
};

// List Hash: bit 31 = 1; always >= 0x8000_0000 > 1 (sentinel-safe)
let list_hash = raw | 0x8000_0000;
```

- `0x0000_0000`: Empty slot sentinel (`HASH_EMPTY`)
- `0x0000_0001`: Tombstone (deleted) sentinel (`HASH_TOMBSTONE`)
- `>= 0x0000_0002`: Valid normalized key hash

### 4.2 Hash Bit-Slicing & 15-Way Folding

The trie routes keys using 4-bit slices of the hash across eight primary levels (`depth % 8`). Each
slice produces a value $S \in [0, 15]$. A value of 15 is mapped to child slot 0, avoiding integer
division,

```rs
let raw_slice = (hash >> (4 * (depth % 8))) & 0x0F;
let child_idx = if raw_slice >= 15 { 0 } else { raw_slice };
```

```
Depth 0: bits  0..3  -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 1: bits  4..7  -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 2: bits  8..11 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 3: bits 12..15 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 4: bits 16..19 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 5: bits 20..23 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 6: bits 24..27 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 7: bits 28..31 -> raw slice 0..15 -> fold (15 -> 0) -> child_idx 0..14
Depth 8+: (depth % 8) cyclically wraps back to bits 0..3
```

### 4.3 Trie Traversal & 32-Entry Leaf Probing

Lookups, inserts, and deletes all start at `ROOT_NODE_SLOT` (1 / `0x01`) with `depth = 0`:

1. Read the current Node slot and obtain `child_idx = trie_child_idx(hash, depth)`
2. Check `(node.flag & (1 << child_idx)) != 0` directly from the parent Node:
   - If `true` (`1` = child Node): set `curr_slot = node.bucket_indices[child_idx]`, increment
     `depth`, and repeat from step 1 using the child Node
   - If `false` (`0` = leaf Bucket): set `target_slot = node.bucket_indices[child_idx]` and proceed
     to leaf probing
3. Read `target_slot` (256 B) and scan all 32 candidate hashes (`hashes[0..31]`):
   - The 32 `u32` hashes occupy 128 bytes and are checked using vectorized SIMD (AVX2 loads 4x
     256-bit vectors; SSE2/NEON load 8x 128-bit vectors, eliminating scalar tail loops)
   - On a hash match, execute the fast 2-buffer read path to verify `klen` and key bytes in DBuf;
     the key is found only if length and bytes match
   - `0x0000_0000` marks an unused slot, so the key is absent
   - `0x0000_0001` marks a tombstone; record the first available index for insertion

### 4.4 Bucket Splitting, Atomic Re-hashing & Depth Guard

A Bucket split is triggered when an insertion targets a Bucket that already contains 32 active
entries,

1. *Depth limit:* Assert `depth < MAX_TRIE_DEPTH` (`0x08`). If the depth limit has been reached,
  return `err::COL` (hash collision limit). This prevents unbounded recursive allocation when hashes
  are identical or otherwise degenerate.
2. *Node allocation:* Acquire `growth_mutex` and inspect Metadata `extra_node_count`:
  - If `extra_node_count > 0`, pop `new_node_slot = extra_nodes[extra_node_count - 1]` and
    decrement `extra_node_count`.
  - If `extra_node_count == 0`, allocate four Node slots (256 B / 4 slots) from `free_slot_marker`
    and advance `free_slot_marker += 4`. Use the first slot as `new_node_slot`, push the remaining
    three slots into `extra_nodes[0..2]`, and set `extra_node_count = 3`.
3. *Bucket allocation and reservation:* Allocate 14 contiguous Buckets
  (56 slots = 3584 B = 14 × 256 B) from `free_slot_marker`, then advance `free_slot_marker += 56`.
  Reserve the Metadata slots (`free_slot_marker` and `extra_nodes`) and format the new child
  component headers in private space while holding `growth_mutex`. Release the mutex when these
  reservations are complete.
4. *Preparation in private allocated space:* Initialize all 14 new Buckets as empty
  (zeroed hashes and offsets); set
  `new_node.flag = 0x0000_0000` (all 15 children initially Buckets),
  `new_node.bucket_indices[0] = existing_bucket_slot`, and
  `new_node.bucket_indices[1..14] = new_bucket_slots[0..13]`; rehash the 32 existing entries and
  the new 33rd entry using the folded 4-bit slice at `depth + 1`; compact entries with
  `child_idx == 0` into the existing Bucket; write entries with `child_idx` in `1..14` to their
  corresponding new Buckets; the new child slots remain beyond the original `free_slot_marker`
  and are therefore invisible to concurrent readers
5. *Atomic OCC publication:* Commit a transaction that validates and updates the following:
   - *Parent Node slot:* Replace the pointer to the existing Bucket with `new_node_slot`, and set
     bit `parent_child_idx` in `parent.flag` (`mutated_parent.flag |= 1 << parent_child_idx`)
   - *Existing Bucket (4 slots):* Replace its contents with the compacted `child_idx == 0` entries
   - *DBuf slots:* Write the allocated key and value payloads.

  Readers move atomically from the old Bucket to the new Node. This prevents partially published
  states and torn reads without requiring write-ahead logging.

## 5. Storage Growth & Allocation Protocols

### 5.1 Monotonic Slot Allocation (256-Byte Quantum)

All internal allocations use integer multiples of 256 B (four slots) and are tracked by
`free_slot_marker` in Metadata (Slot 0),

```
                Database Mapped Memory
┌──────────────────────────┬─────────────────────────────┐
│   Active Allocations     │     Free Mapped Slots       │
└──────────────────────────┴─────────────────────────────┘
0                          ▲                             ▲
                           │                             │
                   free_slot_marker                 total_slots
```

To allocate $K$ slots, where $K$ is an even multiple of four:
1. Acquire the engine `growth_mutex`.
2. Check that `free_slot_marker + K <= total_slots`. If capacity is insufficient, grow the backing
   file by physical OS pages first.
3. Set `allocated_slot = free_slot_marker`.
4. Advance `free_slot_marker += K`.
5. Release `growth_mutex`.
6. Return `allocated_slot`.

### 5.2 Multi-Tx Growth Coordination (Lock-and-Recheck)

Concurrent writers use double-checked locking when capacity is exhausted. The second check prevents
multiple transactions from expanding the file for the same capacity shortfall,

```
Transaction detects capacity exhaustion (DBuf, AdjArr, Node, or Bucket)
  │
  ▼
Acquire engine `growth_mutex`
  │
  ▼
[RE-CHECK] Has capacity been expanded by a preceding transaction?
  ├── YES ──► Release `growth_mutex` ──► Proceed with allocation
  │
  └── NO  (Capacity still exhausted)
        │
        ├─► Call `MemMap::grow(pages)` (remaps backing file under internal locks)
        ├─► Format new components in-place (headers, bitmaps, sentinels)
        ├─► Update Metadata via `mmap.write(0, ...)` (Slot 0: `total_pages`, `total_slots`)
        ├─► Advance `free_slot_marker`
        ├─► Release `growth_mutex`
        └─► Proceed with transaction
```

### 5.3 Component Expansions (256-Byte Aligned Units)

All internal expansions preserve 256 B alignment:

- *Bucket split:* Allocates 14 new Buckets (56 slots = 3584 B = 14 × 256 B), reuses the existing
  Bucket as child index 0, and links the 14 new child Buckets to the newly allocated Node.
- *Node stack replenishment:* When `extra_node_count == 0`, allocates four Nodes
  (4 slots = 256 B = 1 × 256 B), uses one immediately, and stores the remaining three in the
  Metadata stack.
- *DBuf expansion:* Allocates enough DBuf pools for the requested capacity. If
  $P_{\text{needed}} = \lceil K / 31 \rceil$ pools are required, the chain grows by
  $\max(4, 4 \times P_{\text{needed}})$ pools, rounded in groups of four to preserve 4-slot
  alignment. Bitmaps and buffers are zero-initialized, the pools are appended through `next_dbuf`,
  and their slot indices are registered in the in-memory `DBufSlotCache` while holding
  `growth_mutex`.
- *AdjArray expansion:* Allocates two contiguous AdjArray blocks (4 slots = 256 B = 12 list arrays),
  zero-initializes their array bitmaps, links them through `next_adjarr_idx`, and appends them to
  the active AdjArray chain starting at `ADJ_ARR_HEAD_SLOT`.

### 5.4 Physical OS Page Growth Quantization

The backing file is expanded when `free_slot_marker + K > total_slots`:

- *64 KiB page systems:* Grow by one physical page using `mmap.grow(1)`
  (64 KiB = 0x0400 slots = 1024 slots).
- *16 KiB page systems:* Grow by one physical page using `mmap.grow(1)`
  (16 KiB = 0x0100 slots = 256 slots).
- *4 KiB page systems:* Grow by four physical pages using `mmap.grow(4)`
  (16 KiB = 0x0100 slots = 256 slots).

Each expansion adds 256 or 1024 slots. `MemMap::grow` checks that
`new_length <= MAX_DB_LEN` (`0x40_0000_0000`) and returns `err::GRW` if the 256 GiB limit would be
exceeded. It then updates `total_pages` and `total_slots` atomically in Slot 0 through
`mmap.write(0, ...)`.

The `CapacityHint` pre-allocation path uses the same growth quanta: 16 KiB on 4 KiB and 16 KiB
systems, or 64 KiB on 64 KiB systems. This avoids runtime `mmap.grow` calls during the expected
workload.

## 6. Concurrency & Transaction Model (OCC)

Concurrency is managed at the 64 B Slot level using Optimistic Concurrency Control (OCC),

```
Reader:
  1. Read Slot via mmap.read_exact(slot_idx) with torn-read validation
  2. Non-blocking relative to other readers

Writer (Atomic Multi-Slot OCC):
  1. Read required Slots (Bucket hashes/offsets, DBuf bmap/payload, AdjArr, Node)
  2. Compute new Slot contents and bitstates
  3. tx = mmap.new_tx()
  4. Register expected vs new slot values: tx.write(&expected, &new_slot, slot_idx)
  5. Commit:
     a. Sort target slot indices ascending (Deadlock prevention)
     b. Acquire per-slot locks in strictly ascending order
     c. Slot validation: assert mapped slots match expected state
     d. If valid: copy new slots, advance epoch, release locks -> Ok(AckTicket)
     e. If conflict: release locks, abort -> backoff (50 us..10 ms, +/-25% jitter), retry
```

The active entry count (`total_entries`) is maintained in memory using `AtomicU64`. This avoids OCC
write contention on Slot 0 and reduces cache-line bouncing between concurrent transactions. The
value in `Metadata.total_entries` at Slot 0 is flushed during `sync()`.

## 7. Operational Workflows

### 7.1 `get(key: &[u8]) -> Result<Option<Vec<u8>>>`

1. Compute `hash = hash_kv(key)` (`raw & 0x7FFF_FFFF`, normalized $\ge 2$) and initialize
   `curr_slot = ROOT_NODE_SLOT` and `depth = 0`
2. At `curr_slot`, calculate `raw = (hash >> (4 * (depth % 8))) & 0x0F`
3. Calculate `child_idx = if raw >= 15 { 0 } else { raw }`
4. Inspect `(curr_node.flag & (1 << child_idx)) != 0` directly from the current Node:
   - If `true` (`Node`): set `curr_slot = curr_node.bucket_indices[child_idx]`, increment `depth`,
     and return to step 2
   - If `false` (`Bucket`): set `bucket_slot = curr_node.bucket_indices[child_idx]`
5. Read `bucket_slot` (256 B) and scan `hashes[0..31]`:
   - If `hashes[i] == hash`, execute the fast 2-buffer read path at `offsets[i]`
     (`global_buf_idx`); verify `klen == key.len()` and key equality; if match, read value
     payload using `vlen` from header, validate CRC32, and return `Ok(Some(value_bytes))`
   - If `hashes[i] == 0`, return `Ok(None)`
   - If all 32 slots are scanned without a match, return `Ok(None)`

### 7.2 `set(key: &[u8], val: &[u8]) -> Result<TurboTicket>`

1. Validate that `key.len() <= 255` and `val.len() <= 65535`
2. Compute `hash = hash_kv(key)` and traverse the trie to leaf Bucket (Section 4.3)
3. If the key already exists in the Bucket:
   - Read header buffer at `offsets[i]` to inspect `old_vlen` and allocated buffer count
   - If `new_len <= old_allocated`, overwrite the value payload in place through an OCC transaction
   - If the payload grows, allocate new contiguous 8 B buffers in the DBuf chain, write the new
     frame, update `offsets[i]`, and commit through an OCC transaction
4. If the key is not present:
   - If the Bucket contains an empty slot or tombstone, allocate contiguous 8 B buffers from the
     active DBuf chain, write header `[ CRC32 ][ klen ][ vlen ]` and payload
     `[ Key ][ Val ][ Pad ]`, and commit an OCC transaction that updates the DBuf bitmap, DBuf
     buffer slots, Bucket hash, and Bucket offset
   - If the Bucket is full (32 active entries), perform the atomic split described in Section 4.4;
     this allocates 14 child Buckets, rehashes the entries, publishes the new Node, sets the
     parent Node bitmap, and inserts the key

### 7.3 `del(key: &[u8]) -> Result<bool>`

1. Compute `hash = hash_kv(key)` and traverse trie to leaf Bucket (Section 4.3);
   if the key is absent, return `Ok(false)`
2. Commit the deletion atomically through OCC:
   - Set `hashes[i] = 1` (tombstone)
   - Clear the occupied bits in the DBuf `bmap` for all buffers spanned by the entry
   - Decrement the in-memory `total_entries` (`AtomicU64`); the `Metadata.total_entries` value in
     Slot 0 is flushed during `sync()` to avoid writer cache-line contention on Slot 0
3. Return `Ok(true)`

### 7.4 List Operations (`TurboList`)

- The key is registered in the Bucket with bit 31 of hash set to 1 (`hash_list(key)`);
  the entry stores `global_buf_idx` in `offsets[i]`
- Buffer 0 of the DBuf frame stores `[ CRC32 (4 B) ][ klen (1 B) ][ adjarr_idx (3 B) ]`;
  buffers $1..N$ store `[ Key ][ Pad ]`
- Values are stored in chained `AdjArrayList` nodes, with four items per 20 B array
- `append`: Locate the tail array using `adjarr_idx`; if all four entries are occupied,
  allocate a new array from an AdjArr block and link it with `prev.next_arr_idx = new_arr_idx`;
  allocate item buffers using `find_free_databuf_span`, write payload across pools skipping
  trailing metadata, and commit the Bucket (if new list), array block, and affected DBuf slots
  in one atomic transaction
- `pop`: Read the tail item using the item value read process; clear buffer occupancy across all
  spanned pools, decrement array count, and include affected slots in the OCC transaction;
  if the array becomes empty, return it to the block bitmap
- `first` / `last` / `get_at(idx)` / `iter`: Traverse the array chains and load item values through
  DBuf reads across pool boundaries, validating CRC32
- `clear` / `del`: Clear key buffers across every pool they span; traverse the array chain, read
  each item's `vlen`, and clear corresponding item buffers; return arrays to block bitmaps,
  mark the Bucket entry as a tombstone, and commit all affected slots in a single transaction

## 8. Lifecycle & Versioning

### 8.1 Database Instantiation Workflows

OS mapping, file creation, descriptor locking, and page validation are handled by
`MemMap<Slot>` (`Slot: [u8; 64]`):

#### `create(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Query the host OS page size with `page_size = MemMap::<Slot>::page_size()?`.
2. Determine the initial page allocation:
  - If `cfg.capacity_hint` is provided, evaluate `hint.compute_initial_pages(page_size)`. Return
    `err::FUL` if the resulting footprint exceeds `MAX_DB_LEN`.
  - Otherwise, use the default allocation:
    - `PageSize::K4` (4096 B): `initial_pages = 4` (16 KiB = 0x0100 slots).
    - `PageSize::K16` (16384 B): `initial_pages = 1` (16 KiB = 0x0100 slots).
    - `PageSize::K64` (65536 B): `initial_pages = 1` (64 KiB = 0x0400 slots).
3. Construct `MemMapCfg` from `cfg.path`, `DB_NAME_TAG`, and `MAX_DB_LEN`.
4. Create the mapping with `mmap = MemMap::<Slot>::new(memmap_cfg)?`:
   - Return `err::EXS` if the backing file already exists, or `err::DIR` if the directory is invalid
   - Acquire an exclusive filesystem advisory lock on the file descriptor.
   - Reserve virtual address space up to `MAX_DB_LEN`.
5. Format the initial 8192 B layout (Slots 0..0x7F) directly in mapped memory:
   - Slot 0: Metadata (`magic = 0x54424658`, `version = 3`, `page_size`, `total_pages`,
    `total_slots`, `free_slot_marker = 0x80`, `total_entries = 0`,
    `extra_nodes = [124, 125, 126, 127]`, `extra_node_count = 4`).
   - Slot 1: Root Node (`flag = 0x0`, `bucket_indices = [4, 8, 12, ..., 60]`).
   - Slots 2..3: AdjArr head block with zeroed headers and bitmaps, and `next_adjarr_idx = 0`.
   - Slots 4..63: 15 Buckets on Page 0 (zeroed hashes and offsets)
   - Slots 64..123: 15 DBuf pools on Page 1, chained as `64 -> 68 -> ... -> 120 -> 0`.
   - Slots 124..127: Four pre-allocated spare Nodes (`flag = 0x0`, with zeroed child indices).
6. Flush the initial state with `mmap.sync()?` to ensure it is persisted to disk.
7. Resolve pool 0 directly to `DATABUF_HEAD_SLOT`. Resolve subsequent hops on demand, validate them
   on disk, and cache them dynamically.
8. Construct and return the `TurboFox` engine handle.

#### `open(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Query the host OS page size with `sys_page_size = MemMap::<Slot>::page_size()?`.
2. Construct `MemMapCfg` for the target directory, version, and configuration.
3. Open the mapping with `mmap = MemMap::<Slot>::open(memmap_cfg)?`:
   - Return `err::NEX` if the backing file does not exist, or `err::DIR` if the directory is missing
   - Return `err::PTH` if the target path is a directory, or an error if the descriptor lock is held
4. Read Slot 0 Metadata under the reader spinlock using `mmap.read(0, ...)`, then validate the
   following invariants:
   - `meta.magic == 0x54424658` (`"TBFX"`); return an error on mismatch.
   - `meta.version == cfg.version`; return an error if the schema version differs.
   - `meta.page_size == sys_page_size.as_u32()`; return an error if the page sizes differ.
   - `mmap.length() % sys_page_size.as_usize() == 0`; return an error if the file length is not
     page-aligned.
   - `meta.total_slots == (mmap.length() / 64) as u32`; return an error if the value is inconsistent
   - `meta.total_pages == (mmap.length() / sys_page_size.as_usize()) as u32`.
   - `meta.free_slot_marker <= meta.total_slots`; return an error if the value is out of range.
5. Resolve pool 0 directly to `DATABUF_HEAD_SLOT`. Resolve subsequent hops on demand, validate them
   on disk to detect broken chains, and cache them dynamically.
6. Construct and return the `TurboFox` engine handle.

#### `open_or_create(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Construct `MemMapCfg` to resolve the versioned file path (`{dir}/{name}_{version}`).
2. Check whether the backing file exists using `versioned_path().exists()`:
   - If it exists, call `open(cfg)`.
   - If it does not exist, call `create(cfg)`.
3. Propagate all header validation, layout, and advisory-lock errors. Do not recreate the file after
   a validation failure, as this could silently overwrite existing data.

### 8.2 Engine Configuration & `MemMapCfg` Translation

`TurboCfg` exposes the user-facing configuration and maps its parameters to `MemMapCfg`:

#### Constants & Defaults

```rs
pub const DB_NAME_TAG: [u8; 4] = *b"tbfx";
pub const MODULE_ID: u8 = 0x46;
pub const MAX_DB_LEN: usize = 0x40_0000_0000; // 274,877,906,944 B (256 GiB)
```

#### Configuration Mapping

```
TurboCfg Field  Type                  MemMapCfg Target        Resolution Logic
--------------+---------------------+-----------------------+--------------------------------------
path          PathBuf               path                    Forwarded directly; MemMap checks dir
version       u32                   version                 Compiled constant (LAYOUT_VERSION = 3)
sync_mode     SyncMode              sync_interval           Mapped to Periodic(Duration) or Manual
capacity_hint Option<CapacityHint>  initial_pages           Evaluates quantum-aligned initial pages
lookup_cache  bool                  (Engine internal)       In-memory DBufSlotCache (false default)
(constant)    [u8; 4]               name                    DB_NAME_TAG (*b"tbfx")
(constant)    u8                    module_id               MODULE_ID (0x46)
(constant)    usize                 max_len                 MAX_DB_LEN (0x40_0000_0000)
(computed)    usize                 initial_pages           From capacity_hint or default baseline
(flag)        bool                  memory_priority_hint    true (Windows memory priority advice)
(flag)        bool                  sequential_access_hint  true (POSIX_FADV / SEQUENTIAL_SCAN)
```

## 9. Architectural Upper Bounds & Capacity Proof

### 9.1 Addressable Ceilings

The primary architectural limit is imposed by 32-bit slot addressing (`u32`):

- A 32-bit index can address up to $2^{32} = 4{,}294{,}967{,}296$ slots (`0..=0xFFFF_FFFF`).
- At 64 bytes per slot, the maximum addressable database size is 256 GiB (`0x40_0000_0000`):
  $$\text{bytes} = 2^{32} \times 64 = 2^{38} \text{ B} = 274{,}877{,}906{,}944 \text{ B}$$
- Expressed in physical OS pages, this limit is:
  - 4 KiB pages: $2^{38} / 4096 = 2^{26} = 67{,}108{,}864$ pages (`0x0400_0000`)
  - 16 KiB pages: $2^{38} / 16384 = 2^{24} = 16{,}777{,}216$ pages (`0x0100_0000`)
  - 64 KiB pages: $2^{38} / 65536 = 2^{22} = 4{,}194{,}304$ pages (`0x0040_0000`)

### 9.2 Capacity Bounds for Common Payload Formats

- *DBuf pools (4 slots / 256 B per DBuf, with 31 buffers of 8 B):*
  - If the full 256 GiB address space were used for DBuf pools:
    $$2^{32} / 4 = 2^{30} = 1{,}073{,}741{,}824 \text{ pools}$$
    $$1{,}073{,}741{,}824 \times 31 = 33{,}285{,}996{,}544 \text{ buffers}$$
- *KV entries (12 B key + 48 B value $\implies$ 64 B frame $\implies$ 8 buffers):*
  - Each DBuf holds $\lfloor 31 / 8 \rfloor = 3$ entries, leaving 7 buffers unused.
- *List items (20 B value $\implies$ 28 B frame $\implies$ 4 buffers):*
  - Each DBuf holds $\lfloor 31 / 4 \rfloor = 7$ item payloads, leaving 3 buffers unused.
  - The capacity of a chained list is bounded by `next_arr_idx: u32` ($2^{32} - 1 \times 4$ items).

### 9.3 Capacity Derivation

$$
\text{Capacity} = 2^{32} \times 64 \text{ B} = 256 \text{ GiB} = 0\text{x}40\_0000\_0000 \text{ B}
$$
