# Architecture & Technical Specs

TurboFox is a single-file embedded KV storage engine

## 1. Storage Hierarchy & Invariants

```
Unit                 Size      Slots      Alignment  Purpose
-------------------+---------+----------+----------+--------------------------------
Slot (T)           64 B      1          8 B        Base IO / spinlock concurrency
AdjArrayBlock      128 B     2          8 B        1-to-N list index block
Metadata           64 B      1          8 B        Engine configuration, heads, and spare nodes
Node               64 B      1          8 B        Internal 15-way trie routing node
Bucket             256 B     4          8 B        21-entry leaf bucket
DBuf               256 B     4          8 B        Contiguous payload buffer pool
Physical Page 4K   4096 B    64         4096 B     Host filesystem growth quantum
Physical Page 16K  16384 B   256        16384 B    Host filesystem growth quantum
Physical Page 64K  65536 B   1024       65536 B    Host filesystem growth quantum
```

Following invariants must be preserved;

- All disk structures are strictly `#[repr(C)]`, integer multiples of `Slot` (64 B), and 8B aligned
- File growth is strictly quantized to physical OS pages (4 KiB, 16 KiB, or 64 KiB)
- Slot (64 B) is the atomic transaction boundary;
  concurrency uses Optimistic Concurrency Control (OCC);
  ordered multi-slot lock acquisition and validation guarantee deadlock-free transactions
- No virtual page layer exists; all component addressing resolves to absolute slot indices
- Memory allocation inside the mapped file is strictly monotonic bump-pointer allocation;
  bump allocations are quantized to multiples of 256 B (4 slots) to eliminate fragmentation;
  in-place mutation is restricted to atomic Bucket-to-Node split publication and OCC slot writes

## 2. File Topography (Initial Footprint)

Initial structure occupies exactly 8192 B (0x80 slots / 128 slots);
backing file maps 16 KiB (0x0100 slots) on 4 KiB and 16 KiB systems, or 64 KiB (0x0400 slots) on 64K

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

- Initial Index: 1 Root Node at Slot 1 (`ROOT_NODE_SLOT = 0x01`), routing to 15 Buckets
- Initial Payloads: 15 DBuf pools = 465 buffers of 8 B (head `DATABUF_HEAD_SLOT = 0x40`)
- Initial Lists: 1 AdjArray block = 6 list arrays (head `ADJ_ARR_HEAD_SLOT = 0x02`)
- Initial Spare Nodes: 4 pre-allocated Nodes at Slots 124..127 tracked in Metadata
- Initial Unallocated: Slots 0x80..0xFF (16 KiB mapping) or Slots 0x80..0x03FF (64 KiB mapping);
  `free_slot_marker` initializes to 128 (`0x80`), perfectly aligned to 256 B multiples

## 3. Component Memory Layouts

### 3.1 Static Topography Constants & Metadata (Slot 0, 64 Bytes, `align(8)`)

#### Engine Static Head Constants

Base component head slot indices remain constant across the database lifetime;
hardcoded as immutable compile-time constants:

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

Internal trie node indexing up to 15 child buckets or lower-level nodes;
occupies exactly 1 slot (64 B):

```
Offset   Type        Field           Description
-------+-----------+---------------+--------------------------------------------------
0..3     u32         flag            Discriminator flag (0x0000_0000 = Node)
4..63    [u32; 15]   bucket_indices  Slot indices of 15 child Buckets or Nodes
```

- `flag`: Always `0x0000_0000` for `Node`; identifies node type during branch traversal
- `bucket_indices[i]`: Absolute slot index of child component for slice $i \in [0, 14]$

### 3.3 Bucket (256 Bytes, 4 Slots, `align(8)`)

Leaf hash table slot holding up to 21 entries;
occupies exactly 4 slots (256 B):

```
Offset   Type        Field    Description
-------+-----------+--------+---------------------------------------------------------
0..3     u32         flag     Discriminator flag (0x0000_0001 = Bucket)
4..87    [u32; 21]   hashes   Normalized key hashes (0 = empty, 1 = tombstone, >= 2)
88..255  [u64; 21]   offsets  Packed entry metadata (flags, lengths, buffer index)
```

#### Hash Sentinels (`hashes[i]`)

- `0x0000_0000`: Empty slot sentinel
- `0x0000_0001`: Tombstone (deleted) sentinel
- `>= 0x0000_0002`: Valid normalized XxHash32 hash

#### Entry Offset Encoding (`offsets[i]`, 8 Bytes)

Packed 8-byte representation distinguishing KV entries from List heads;
the very first bit in memory (Byte 0, Bit 7 / mask `0x80`) is the discriminator flag:

- `offsets[i][0] & 0x80 == 0x00`: **KV Entry**
- `offsets[i][0] & 0x80 == 0x80`: **List Entry**

**KV Entry Format (8 Bytes)**:
```
Byte 0          Byte 1      Bytes 2..3 (LE)  Bytes 4..7 (LE)
+-------------+-----------+----------------+------------------------+
| Flags (0x0) | klen (u8) | vlen (u16)     | global_buf_idx (u32)   |
+-------------+-----------+----------------+------------------------+
```
- `Byte 0 (flags)`: Bit 7 is `0` (`isKV`); bits 0..6 reserved (`0x00`)
- `Byte 1 (klen)`: Key byte length (1..255)
- `Bytes 2..3 (vlen)`: Value byte length in little-endian (0..65535)
- `Bytes 4..7 (global_buf_idx)`: Global 8 B DBuf buffer index in little-endian

**List Entry Format (8 Bytes)**:
```
Bytes 0..3 (Big-Endian u32)                    Bytes 4..7 (LE)
+--------------------------------------------+------------------------+
| Flag (Bit 31 = 1) | adjarr_idx (Bits 0..30) | global_buf_idx (u32)  |
+--------------------------------------------+------------------------+
```
- `Bytes 0..3 (flag | adjarr_idx)`:
  Packed as Big-Endian `u32` (Bit 31 = 1, Bits 0..30 = relative array index);
  evaluates identically as `(bytes[0] & 0x80) != 0`
- `Bytes 4..7 (global_buf_idx)`: Global 8 B DBuf buffer index holding key framing

### 3.4 DBuf (256 Bytes, 4 Slots, `align(8)`)

Contiguous pool of 31 8-byte payload buffers;
occupies exactly 4 slots (256 B):

```
Offset    Type            Field      Description
--------+---------------+----------+--------------------------------------------------
0..247    [[u8; 8]; 31]   buffers    31 payload buffers (8 B each = 248 B)
248..251  u32             next_dbuf  Slot index of next DBuf in chain (0 = tail)
252..255  u32             bmap       Bits 0..30: buffer occupancy; Bit 31: pool full flag
```

- `buffers`: 31 discrete 8-byte buffers for raw key and value payloads
- `next_dbuf`: Absolute slot index of next DBuf pool in monotonic allocation chain
- `bmap`: Bitmask where bit $i$ ($0 \le i \le 30$) tracks occupancy of `buffers[i]`;
  MSB (bit 31, mask `0x8000_0000`) is set when all 31 buffers are occupied,
  enabling single-cycle full-pool branch evaluation (`(bmap as i32) < 0`)

#### Payload Framing (8-Byte Alignment)

Contiguous buffers allocate blocks according to total framed length:

- **KV Framing**:
  Allocates $\lceil (4 + \text{klen} + \text{vlen}) / 8 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ Key Bytes (klen) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```
- **List Key Framing**:
  Allocates $\lceil (5 + \text{klen}) / 8 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ klen (1 B) ][ Key Bytes (klen) ][ 0-Padding ]
  ```
- **List Item Value Framing**:
  Allocates $\lceil (6 + \text{vlen}) / 8 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ vlen (2 B, LE) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```

#### Two-Phase Read Process (List Key and List Item Value)

- **List Key Read**:
  1. Read single 8 B buffer at `dbuf_idx`; contains `[ CRC32 (4 B) ][ klen (1 B) ][ Key[0..2] ]`
  2. Inspect `klen` (Byte 4); if `klen <= 3`, key payload resides entirely within initial buffer
  3. If `klen > 3`, compute remaining buffers: $N = \lceil (\text{klen} - 3) / 8 \rceil$;
     read exactly $N$ contiguous buffers starting at `dbuf_idx + 1`
  4. Concatenate bytes and verify CRC32
- **List Item Value Read**:
  1. Read single 8 B buffer at `dbuf_idx`; contains `[ CRC32 (4 B) ][ vlen (2 B) ][ Val[0..1] ]`
  2. Inspect `vlen` (Bytes 4..5, LE); if `vlen <= 2`, value resides entirely within initial buffer
  3. If `vlen > 2`, compute remaining buffers: $N = \lceil (\text{vlen} - 2) / 8 \rceil$;
     read exactly $N$ contiguous buffers starting at `dbuf_idx + 1`
  4. Concatenate bytes and verify CRC32

Global buffer address translation;
for global buffer index $B$:
$$\text{pool\\_hop} = \lfloor B / 31 \rfloor$$
$$\text{local\\_buf} = B \pmod{31}$$

#### In-Memory DBuf Slot Cache (Wait-Free $O(1)$ Hop Resolution)

Translating `pool_hop` to a physical slot index via the persistent `next_dbuf` pointer
chain requires $O(\text{pool\_hop})$ sequential disk/memory loads;
an in-memory append-only chunked array provides wait-free $O(1)$ resolution:

```rs
const CHUNK_SHIFT: usize = 0x08; // 256 entries per chunk (1 KiB chunk)
const CHUNK_SIZE: usize = 1 << CHUNK_SHIFT;
const CHUNK_MASK: usize = CHUNK_SIZE - 1;
const MAX_CHUNKS: usize = 0x0100; // 256 chunks covers 65,536 DBuf pools (16 MiB DB)

pub(crate) struct DBufSlotCache {
    chunks: [AtomicPtr<[AtomicU32; CHUNK_SIZE]>; MAX_CHUNKS],
}
```

- **Read Path**:
  Chunk index and intra-chunk offset resolved via bitwise shifts:
  `chunk_idx = (hop as usize) >> CHUNK_SHIFT;`
  `offset = (hop as usize) & CHUNK_MASK;`
  Loads chunk pointer with `Ordering::Acquire`; reads slot with zero locks or refcounts;
  wait-free execution prevents cache-line bouncing across concurrent transactions;
  pool 0 resolves immediately to `DATABUF_HEAD_SLOT` via constant fast-path
- **Write Path (Growth)**:
  Under `growth_mutex`, when crossing chunk boundary, allocates 1 KiB boxed chunk;
  publishes pointer with `Ordering::Release`; writes slot indices with monotonic stores
- **Footprint**:
  2 KiB outer pointer array (`256 * 8 B`); allocates 1 KiB lazily per 256 DBuf pools;
  consumes exactly 4 B per DBuf pool (1.56% of raw DBuf storage);
  pool 0 fast-path avoids allocating chunk 0 at initialization

### 3.5 Adjacent Array Block (128 Bytes, 2 Slots, `align(8)`)

Index structure for 1-to-N list pointer chains;
occupies exactly 2 slots (128 B)

```
Offset    Type                 Field            Description
--------+--------------------+----------------+---------------------------------------
0..119    [AdjArrayList; 6]    arrays           6 list arrays (20 B each = 120 B)
120..123  u32                  next_adjarr_idx  Slot index of next AdjArr block in chain
124..125  u16                  bmap             Bits 0..5: array occupancy mask (0x3F)
126..127  [u8; 2]              padding          Zero alignment padding to 128 B
```

#### `AdjArrayEntry` (4 Bytes, `repr(C)`)

- `dbuf_idx: u32` (4 B): Global DBuf buffer index holding `[CRC32][vlen][Value]`

#### `AdjArrayList` (20 Bytes, `repr(C)`)

- `entries: [AdjArrayEntry; 4]` (16 B): 4 list item pointers
- `next_arr_idx: u32` (4 B): Global relative index of next array in chain (`0` = tail)

#### Global Relative Array Indexing

For relative array index $A$:
$$\text{block\\_hop} = \lfloor A / 6 \rfloor$$
$$\text{local\\_array} = A \pmod{6}$$

Traverse `next_adjarr_idx` block chain by `block_hop` steps;
index into `arrays[local_array]`

## 4. Hash Routing, Trie Traversal & Splitting

### 4.1 Key Hash Normalization

Algorithm: `twox_hash::XxHash32` initialized with seed `0x00` (as default SEED is fastest option)

```rs
let h = hasher.finish() as u32;
let hash = if h < 2 { h + 2 } else { h };
```

- `0`: Empty slot sentinel
- `1`: Tombstone (deleted) sentinel
- `>= 2`: Valid normalized key hash

### 4.2 Hash Bit-Slicing & 15-Way Folding

Key hashes route through trie levels via 4-bit slices across 8 primary levels (`depth % 8`);
raw 4-bit slices yield values $S \in [0, 15]$;
value 15 folds into child slot 0, eliminating integer division:

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

### 4.3 Trie Traversal & 21-Entry Leaf Probing

Every lookup, insert, and delete begins at `ROOT_NODE_SLOT` (1 / `0x01`) at `depth = 0`:

1. Read current Node slot; extract `target_slot = bucket_indices[child_idx]`
2. Read slot at `target_slot`; inspect `flag` at Byte 0:
   - If `flag == 0x0000_0000` (`Node`):
     Advance `depth += 1`; repeat step 1 at child Node
   - If `flag == 0x0000_0001` (`Bucket`):
     Scan all 21 candidate hashes (`hashes[0..20]`) directly:
     - 21 `u32` hashes span 84 bytes; evaluated via vectorized or unrolled scalar scanning
     - Match: compare key bytes in DBuf; if identical, key found
     - Sentinel `0x0000_0000`: unallocated slot; key is missing
     - Sentinel `0x0000_0001`: tombstone encountered; record first free index for insertion

### 4.4 Bucket Splitting, Atomic Re-hashing & Depth Guard

When an insertion targets a Bucket containing 21 active entries:

1. **Depth Ceiling Guard**:
   Assert `depth < MAX_TRIE_DEPTH` (`0x08`);
   if depth ceiling is reached, fail with `err::COL` (hash collision limit);
   prevents runaway recursive allocation loops on degenerate identical hashes
2. **Node Sourcing & Extra Node Stack**:
   Acquire `growth_mutex`;
   inspect Metadata `extra_node_count`:
   - If `extra_node_count > 0`: pop `new_node_slot = extra_nodes[extra_node_count - 1]`;
     decrement `extra_node_count`
   - If `extra_node_count == 0`: allocate 4 new Node slots (256 B / 4 slots)
     from `free_slot_marker`; advance `free_slot_marker += 4`;
     `new_node_slot` becomes the first allocated slot;
     push remaining 3 slots into `extra_nodes[0..2]`; set `extra_node_count = 3`
3. **Contiguous Bucket Allocation & Storage Reservation**:
   Allocate 14 contiguous new Buckets (56 slots = 3584 B = 14 x 256 B) from `free_slot_marker`;
   advance `free_slot_marker += 56`;
   Metadata slot reservations (`free_slot_marker`, `extra_nodes`) and new child component
   headers are formatted in private space under `growth_mutex`;
   release `growth_mutex`
4. **Pre-Formatting in Private Allocated Space**:
   Format all 14 new buckets as empty (`flag = 0x0000_0001`, zeroed hashes and offsets);
   new Node assigns `bucket_indices[0] = existing_bucket_slot`;
   `bucket_indices[1..14] = new_bucket_slots[0..13]`;
   rehash the 21 existing entries plus the new 22nd entry using `depth + 1` 4-bit folded slice;
   entries with `child_idx == 0` are compacted into the existing bucket;
   entries with `child_idx in 1..14` write into target newly allocated buckets;
   new child slots reside beyond original `free_slot_marker`, invisible to concurrent readers
5. **Atomic OCC Publication**:
   Commit transaction validating and updating:
   - Parent Node slot: replace child pointer to existing bucket with `new_node_slot`
   - Existing Bucket (4 slots): overwrite with compacted `child_idx == 0` entries
   - DBuf slots: write allocated key/val payloads;
   readers transition atomically from reading the old bucket to traversing the new Node;
   guarantees zero partial states or torn reads without write-ahead logging

## 5. Storage Growth & Allocation Protocols

### 5.1 Monotonic Slot Allocation (256-Byte Quantum)

All internal allocations consume uniform integer multiples of 256 B (4 slots);
managed by `free_slot_marker` in Metadata (Slot 0):

```
                Database Mapped Memory
┌──────────────────────────┬─────────────────────────────┐
│   Active Allocations     │     Free Mapped Slots       │
└──────────────────────────┴─────────────────────────────┘
0                          ▲                             ▲
                           │                             │
                   free_slot_marker                 total_slots
```

To allocate $K$ slots (where $K$ is an even multiple of 4):
1. Acquire engine `growth_mutex`
2. Assert `free_slot_marker + K <= total_slots`;
   if capacity exceeded, invoke physical OS page growth first
3. `allocated_slot = free_slot_marker`
4. `free_slot_marker += K`
5. Release `growth_mutex`
6. Return `allocated_slot`

### 5.2 Multi-Tx Growth Coordination (Lock-and-Recheck)

Concurrent writers handle capacity exhaustion via Double-Checked Locking;
prevents allocation stampedes and duplicate file expansion:

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

Every internal expansion strictly preserves 256 B alignment:

- **Bucket Split Expansion**:
  Allocates 14 new Buckets (56 slots = 3584 B = 14 x 256 B);
  reuses existing bucket as child index 0;
  links 14 new child buckets to newly acquired Node
- **Node Stack Replenishment**:
  When `extra_node_count == 0`, allocates 4 new Nodes (4 slots = 256 B = 1 x 256 B);
  uses 1 immediately and caches 3 in Metadata stack
- **DBuf Expansion**:
  Allocates 4 new DBuf pools (16 slots = 1024 B = 4 x 256 B = 124 buffers of 8 B);
  zero-initializes bitmaps and buffers;
  appends to DBuf allocation chain via `next_dbuf`;
  registers new slot indices into in-memory `DBufSlotCache` under `growth_mutex`
- **AdjArray Expansion**:
  Allocates 2 contiguous AdjArray blocks (4 slots = 256 B = 12 list arrays);
  zero-initializes array bitmaps;
  links blocks together via `next_adjarr_idx`;
  appends to active AdjArray chain originating at `ADJ_ARR_HEAD_SLOT`

### 5.4 Physical OS Page Growth Quantization

When `free_slot_marker + K > total_slots`, backing file expands:

- **64 KiB OS Page Systems**:
  Expand by 1 physical page: `mmap.grow(1)` (64 KiB = 0x0400 slots = 1024 slots)
- **16 KiB OS Page Systems**:
  Expand by 1 physical page: `mmap.grow(1)` (16 KiB = 0x0100 slots = 256 slots)
- **4 KiB OS Page Systems**:
  Expand by 4 physical pages: `mmap.grow(4)` (16 KiB = 0x0100 slots = 256 slots)

Each expansion appends 256 slots (or 1024 slots);
`MemMap::grow` validates that `new_length <= MAX_DB_LEN` (`0x40_0000_0000`);
fails with `err::GRW` if the 256 GiB ceiling is breached;
advances `total_pages` and `total_slots` atomically in Slot 0 via `mmap.write(0, ...)`;

Pre-allocation through `CapacityHint` aligns upfront backing storage allocations to identical
growth quanta (16 KiB on 4 KiB/16 KiB OS systems; 64 KiB on 64 KiB OS systems), eliminating
runtime `mmap.grow` invocations during expected workload operations

## 6. Concurrency & Transaction Model (OCC)

Concurrency executes at Slot granularity (64 B) using Optimistic Concurrency Control:

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

Active entry counts (`total_entries`) are maintained in memory via `AtomicU64` to prevent
Slot 0 OCC write contention and cache-line bouncing across concurrent transactions;
Slot 0 `Metadata.total_entries` is flushed on `sync()`

## 7. Operational Workflows

### 7.1 `get(key: &[u8]) -> Result<Option<Vec<u8>>>`

1. Compute `hash = hash_key(key)`; initialize `curr_slot = ROOT_NODE_SLOT`, `depth = 0`
2. At `curr_slot`, evaluate `raw = (hash >> (4 * (depth % 8))) & 0x0F`
3. `child_idx = if raw >= 15 { 0 } else { raw }`
4. Read slot at `next_slot = node.bucket_indices[child_idx]`
5. If `flag == 0x0000_0000` (`Node`):
   Set `curr_slot = next_slot`, `depth = depth + 1`, loop to step 2
6. If `flag == 0x0000_0001` (`Bucket`):
   Scan `hashes[0..20]`:
   - If `hashes[i] == hash`:
     Inspect `offsets[i]`:
     if byte 0 bit 7 is set (`offsets[i][0] & 0x80 != 0`):
       return `Err(InvalidType("Key is a list"))`
     Read payload from DBuf buffer `global_buf_idx`;
     validate CRC32 and key equality;
     return `Ok(Some(value_bytes))`
   - If `hashes[i] == 0`:
     Return `Ok(None)`
   Return `Ok(None)` if all 21 slots scanned without match

### 7.2 `set(key: &[u8], val: &[u8]) -> Result<TurboTicket>`

1. Validate `key.len() <= 255` and `val.len() <= 65535`
2. Traverse trie to leaf Bucket following Section 4.3
3. If key exists in Bucket:
   - If `new_len <= old_allocated`: overwrite value payload in-place via OCC tx
   - If payload expands: allocate new contiguous 8 B buffers in DBuf chain;
     write new frame; update `offsets[i]`; commit via OCC tx
4. If key is missing:
   - If Bucket has an empty slot or tombstone:
     Allocate contiguous 8 B buffers in active DBuf chain;
     write payload frame `[ CRC32 ][ Key ][ Val ][ Pad ]`;
     commit OCC tx writing DBuf bitmap, DBuf buffer slots, Bucket hash, Bucket offset
   - If Bucket is full (21 active entries):
     Execute Section 4.4 atomic bucket split;
     allocates 14 child buckets, rehashes entries, publishes new Node, and inserts key

### 7.3 `del(key: &[u8]) -> Result<bool>`

1. Traverse trie to leaf Bucket following Section 4.3;
   if key absent, return `Ok(false)`
2. Atomic OCC commit:
   - Set `hashes[i] = 1` (tombstone)
   - Clear occupied bits in DBuf `bmap`
   - Decrement in-memory `total_entries` (`AtomicU64`); Slot 0 `Metadata.total_entries`
     is flushed on `sync()` to avoid writer cache-line contention on Slot 0
3. Return `Ok(true)`

### 7.4 List Operations (`TurboList`)

- Key registered in Bucket with byte 0 bit 7 set to 1 (`offsets[i][0] & 0x80 != 0`);
  stores `adjarr_idx` (relative array index) and `global_buf_idx` (key payload)
- DBuf stores `[ CRC32 ][ klen (1 B) ][ Key ][ Pad ]`
- Values stored across chained `AdjArrayList` nodes (4 items per 20 B array)
- `append`:
  Locate tail array;
  if all 4 entries occupied, allocate new array from AdjArr block;
  link `prev.next_arr_idx = new_arr_idx`
- `pop`:
  Read tail item via two-phase read, clear DBuf buffers, decrement array count;
  if array becomes empty, return to block bitmap
- `first` / `last` / `get_at(idx)` / `iter`:
  Traverse array chains;
  load item values via two-phase DBuf buffer reads, validating CRC32 per item

## 8. Lifecycle & Versioning

### 8.1 Database Instantiation Workflows

Initialization delegates OS mapping, file creation, descriptor locking,
and page validation to `MemMap<Slot>` (`Slot: [u8; 64]`):

#### `create(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Query host OS page size via `page_size = MemMap::<Slot>::page_size()?`
2. Determine initial allocation pages:
   - When `cfg.capacity_hint` is provided: evaluate `hint.compute_initial_pages(page_size)`;
     fails with `err::FUL` if computed footprint exceeds `MAX_DB_LEN`
   - Otherwise, default to baseline:
     - `PageSize::K4` (4096 B): `initial_pages = 4` (16 KiB = 0x0100 slots)
     - `PageSize::K16` (16384 B): `initial_pages = 1` (16 KiB = 0x0100 slots)
     - `PageSize::K64` (65536 B): `initial_pages = 1` (64 KiB = 0x0400 slots)
3. Construct `MemMapCfg` using `cfg.path`, `DB_NAME_TAG`, and `MAX_DB_LEN`
4. Call `mmap = MemMap::<Slot>::new(memmap_cfg)?`:
   - Fails if backing file already exists (`err::EXS`) or directory is invalid (`err::DIR`)
   - Acquires exclusive filesystem advisory lock on file descriptor
   - Reserves virtual address space up to `MAX_DB_LEN`
5. Format initial 8192 B topography (Slots 0..0x7F) directly into mapped memory:
   - Slot 0: Metadata (`magic = 0x54424658`, `version = 3`, `page_size`, `total_pages`,
     `total_slots`, `free_slot_marker = 0x80`, `total_entries = 0`,
     `extra_nodes = [124, 125, 126, 127]`, `extra_node_count = 4`)
   - Slot 1: Root Node (`flag = 0x0`, `bucket_indices = [4, 8, 12, ..., 60]`)
   - Slots 2..3: AdjArr head block (zeroed headers, bitmaps, and `next_adjarr_idx = 0`)
   - Slots 4..63: 15x Buckets on Page 0 (`flag = 0x1`, zeroed hashes and offsets)
   - Slots 64..123: 15x DBuf pools on Page 1 (chained `64 -> 68 -> ... -> 120 -> 0`)
   - Slots 124..127: 4x Pre-allocated spare Nodes (`flag = 0x0`, zeroed child indices)
6. Flush initial state via `mmap.sync()?` to guarantee disk persistence
7. Pool 0 resolves directly to `DATABUF_HEAD_SLOT`;
   subsequent hops resolve, validate on disk, and cache dynamically on demand
8. Construct and return `TurboFox` engine handle

#### `open(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Query host OS page size via `sys_page_size = MemMap::<Slot>::page_size()?`
2. Construct `MemMapCfg` matching target directory, version, and configuration
3. Call `mmap = MemMap::<Slot>::open(memmap_cfg)?`:
   - Fails if backing file does not exist (`err::NEX`) or directory is missing (`err::DIR`)
   - Fails if target path is a directory (`err::PTH`) or descriptor lock is held
4. Read Slot 0 Metadata under reader spinlock via `mmap.read(0, ...)` and validate invariants:
   - Assert `meta.magic == 0x54424658` (`"TBFX"`); error on mismatch
   - Assert `meta.version == cfg.version`; error on schema version mismatch
   - Assert `meta.page_size == sys_page_size.as_u32()`; error on divergence
   - Assert `mmap.length() % sys_page_size.as_usize() == 0`; error if unpadded
   - Assert `meta.total_slots == (mmap.length() / 64) as u32`; error if corrupt
   - Assert `meta.total_pages == (mmap.length() / sys_page_size.as_usize()) as u32`
   - Assert `meta.free_slot_marker <= meta.total_slots`; error if corrupt
5. Pool 0 resolves directly to `DATABUF_HEAD_SLOT`;
   subsequent hops resolve, validate on disk for broken chain detection, and cache dynamically
6. Construct and return `TurboFox` engine handle

#### `open_or_create(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Construct `MemMapCfg` resolving target versioned file path (`{dir}/{name}_{version}`)
2. Check backing storage presence via `versioned_path().exists()`:
   - If present: delegate directly to `open(cfg)`
   - If absent: delegate directly to `create(cfg)`
3. Strictly propagate all header verification, layout, and advisory lock errors;
   never fall back to recreation on validation failure to prevent silent data clobbering

### 8.2 Engine Configuration & `MemMapCfg` Translation

`TurboCfg` exposes user parameters and translates directly to `MemMapCfg`:

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

The primary architectural ceiling stems from 32-bit slot addressing (`u32`):

- 32 bits address up to $2^{32} = 4{,}294{,}967{,}296$ slots (`0..=0xFFFF_FFFF`)
- At 64 bytes per slot, maximum addressable database file size is 256 GiB (`0x40_0000_0000`):
  $$\text{bytes} = 2^{32} \times 64 = 2^{38} \text{ B} = 274{,}877{,}906{,}944 \text{ B}$$
- Quantized to physical OS pages:
  - 4 KiB pages: $2^{38} / 4096 = 2^{26} = 67{,}108{,}864$ pages (`0x0400_0000`)
  - 16 KiB pages: $2^{38} / 16384 = 2^{24} = 16{,}777{,}216$ pages (`0x0100_0000`)
  - 64 KiB pages: $2^{38} / 65536 = 2^{22} = 4{,}194{,}304$ pages (`0x0040_0000`)

### 9.2 Capacity Bounds for Common Payload Formats

- **DBuf Pools (4 slots / 256 B per DBuf, 31 buffers of 8 B)**:
  - If entire 256 GiB space were dedicated to DBuf pools:
    $$2^{32} / 4 = 2^{30} = 1{,}073{,}741{,}824 \text{ pools}$$
    $$1{,}073{,}741{,}824 \times 31 = 33{,}285{,}996{,}544 \text{ buffers}$$
- **KV Entries (12 B key + 48 B value $\implies$ 64 B frame $\implies$ 8 buffers)**:
  - Each DBuf fits $\lfloor 31 / 8 \rfloor = 3$ entries (7 slack buffers)
- **List Items (20 B value $\implies$ 28 B frame $\implies$ 4 buffers)**:
  - Each DBuf fits $\lfloor 31 / 4 \rfloor = 7$ item payloads (3 slack buffers)
  - Chained list item capacity: bounded by `next_arr_idx: u32` ($2^{32} - 1 \times 4$ items)

### 9.3 Capacity Derivation

$$
\text{Capacity} = 2^{32} \times 64 \text{ B} = 256 \text{ GiB} = 0\text{x}40\_0000\_0000 \text{ B}
$$
