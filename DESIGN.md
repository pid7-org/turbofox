# Architecture & Technical Specs

TurboFox is a single-file embedded KV storage engine

## 1. Storage Hierarchy & Invariants

```
Unit                 Size      Slots      Alignment  Purpose
-------------------+---------+----------+----------+--------------------------------
Slot (T)           128 B     1          8 B        Base IO / spinlock concurrency
AdjArrayBlock      128 B     1          8 B        1-to-N list index block
Metadata           128 B     1          8 B        Engine configuration and heads
Node               128 B     1          8 B        Internal 30-way trie routing node
Bucket             128 B     1          8 B        10-entry leaf bucket
DBuf               256 B     2          8 B        Contiguous payload buffer pool
Physical Page 4K   4096 B    32         4096 B     Host filesystem growth quantum
Physical Page 16K  16384 B   128        16384 B    Host filesystem growth quantum
Physical Page 64K  65536 B   512        65536 B    Host filesystem growth quantum
```

Following invariants must be preserved;

- All disk structures are strictly `#[repr(C)]`, integer multiples of `Slot` (128 B), and 8B aligned
- File growth is strictly quantized to physical OS pages (4 KiB, 16 KiB, or 64 KiB)
- Slot (128 B) is the atomic transaction boundary;
  concurrency uses Optimistic Concurrency Control (OCC);
  ordered multi-slot lock acquisition and validation guarantee deadlock-free transactions
- No virtual page layer exists; all component addressing resolves to absolute slot indices
- Memory allocation inside the mapped file is strictly monotonic bump-pointer allocation;
  bump allocations are quantized to multiples of 256 B (2 slots) to eliminate fragmentation;
  in-place mutation is strictly restricted to atomic Bucket-to-Node split publication

## 2. File Topography (Initial Footprint)

Initial structure occupies exactly 6144 B (0x30 slots / 48 slots);
backing file maps 16 KiB (0x80 slots) on 4 KiB and 16 KiB systems, or 64 KiB (0x0200 slots) on 64K

```
Slot Range   Byte Range     Size     Component
-----------+--------------+--------+--------------------------------------------------
0            0x0000..0x007F 128 B    Slot 0: Metadata
1            0x0080..0x00FF 128 B    Slot 1: AdjArrayBlock head (ADJ_ARR_HEAD_SLOT)
2..15        0x0100..0x07FF 1792 B   7x DBuf pools (256 B each, 2 slots each)
16           0x0800..0x087F 128 B    Slot 16: Root Node (ROOT_NODE_SLOT)
17..31       0x0880..0x0FFF 1920 B   15x Initial Buckets (child indices 0x00..0x0E)
32..46       0x1000..0x177F 1920 B   15x Initial Buckets (child indices 0x0F..0x1D)
47           0x1780..0x17FF 128 B    Slot 47: AdjArrayBlock tail (linked from Slot 1)
48..N        0x1800..EOF    Rest     Unallocated slots managed by free_slot_marker
```

- Initial Index: 1 Root Node at Slot 16 (`ROOT_NODE_SLOT = 0x10`), routing to 30 Buckets
- Initial Payloads: 7 DBuf pools = 217 buffers of 8 B (head `DATABUF_HEAD_SLOT = 0x02`)
- Initial Lists: 2 AdjArray blocks = 12 list arrays (head `ADJ_ARR_HEAD_SLOT = 0x01`)
- Initial Unallocated: Slots 0x30..0x7F (16 KiB mapping) or Slots 0x30..0x01FF (64 KiB mapping);
  `free_slot_marker` initializes to 48 (`0x30`), perfectly aligned to 256 B multiples

## 3. Component Memory Layouts

### 3.1 Static Topography Constants & Metadata (Slot 0, 128 Bytes, `align(8)`)

#### Engine Static Head Constants

Base component head slot indices remain constant across the database lifetime;
hardcoded as immutable compile-time constants:

```rs
pub const ROOT_NODE_SLOT: u32 = 0x10;    // Slot 16 (Byte offset 0x0800)
pub const ADJ_ARR_HEAD_SLOT: u32 = 0x01; // Slot 1  (Byte offset 0x0080)
pub const DATABUF_HEAD_SLOT: u32 = 0x02; // Slot 2  (Byte offset 0x0100)
pub const MAX_TRIE_DEPTH: usize = 0x07;  // Depth ceiling guarding hash collisions
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
20..23   u32       free_slot_marker  Next unallocated slot index (starts at 48 / 0x30)
24..31   u64       total_entries     Active key count (KV + List)
32..127  [u8; 96]  reserved          Zero-padded reserved space
```

### 3.2 Node (128 Bytes, 1 Slot, `align(8)`)

Internal trie node indexing up to 30 child buckets or lower-level nodes;
occupies exactly 1 slot (128 B):

```
Offset   Type        Field           Description
-------+-----------+---------------+--------------------------------------------------
0..3     u32         flag            Discriminator flag (0x0000_0000 = Node)
4..7     [u8; 4]     padding         Zero alignment padding to 8 B
8..127   [u32; 30]   bucket_indices  Slot indices of 30 child Buckets or Nodes
```

- `flag`: Always `0x0000_0000` for `Node`; identifies node type during branch traversal
- `bucket_indices[i]`: Absolute slot index of child component for slice $i \in [0, 29]$

### 3.3 Bucket (128 Bytes, 1 Slot, `align(8)`)

Leaf hash table slot holding up to 10 entries;
occupies exactly 1 slot (128 B):

```
Offset   Type        Field    Description
-------+-----------+--------+---------------------------------------------------------
0..3     u32         flag     Discriminator flag (0x0000_0001 = Bucket)
4..7     [u8; 4]     padding  Zero alignment padding to 8 B
8..47    [u32; 10]   hashes   Normalized key hashes (0 = empty, 1 = tombstone, >= 2)
48..127  [u64; 10]   offsets  Packed entry metadata (flags, lengths, buffer index)
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

### 3.4 DBuf (256 Bytes, 2 Slots, `align(8)`)

Contiguous pool of 31 8-byte payload buffers;
occupies exactly 2 slots (256 B):

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

### 3.5 Adjacent Array Block (128 Bytes, 1 Slot, `align(8)`)

Index structure for 1-to-N list pointer chains;
occupies exactly 1 slot (128 B)

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

Algorithm: `twox_hash::XxHash32` initialized with seed `0x5442_4658` (`"TBFX"`)

```rs
let h = hasher.finish() as u32;
let hash = if h < 2 { h + 2 } else { h };
```

- `0`: Empty slot sentinel
- `1`: Tombstone (deleted) sentinel
- `>= 2`: Valid normalized key hash

### 4.2 Hash Bit-Slicing & 30-Way Folding

Key hashes route through trie levels via 5-bit slices (bits 0..29 across 6 primary levels);
raw 5-bit slices yield values $S \in [0, 31]$;
values 30 and 31 fold into slots 0 and 1, eliminating integer division:

```rs
let raw_slice = (hash >> (5 * (depth % 6))) & 0x1F;
let child_idx = if raw_slice >= 30 { raw_slice - 30 } else { raw_slice };
```

```
Depth 0: bits  0..4  -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 1: bits  5..9  -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 2: bits 10..14 -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 3: bits 15..19 -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 4: bits 20..24 -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 5: bits 25..29 -> raw slice 0..31 -> fold (30,31 -> 0,1) -> child_idx 0..29
Depth 6+: (depth % 6) cyclically wraps back to bits 0..4
```

### 4.3 Trie Traversal & 10-Entry Leaf Probing

Every lookup, insert, and delete begins at `ROOT_NODE_SLOT` (16 / `0x10`) at `depth = 0`:

1. Read current Node slot; extract `target_slot = bucket_indices[child_idx]`
2. Read slot at `target_slot`; inspect `flag` at Byte 0:
   - If `flag == 0x0000_0000` (`Node`):
     Advance `depth += 1`; repeat step 1 at child Node
   - If `flag == 0x0000_0001` (`Bucket`):
     Scan all 10 candidate hashes (`hashes[0..9]`) directly:
     - 10 `u32` hashes span exactly 40 bytes (resides entirely within one 64 B cache line);
       probed via unrolled scalar scan or 128-bit vector comparisons without branch penalties
     - Match: compare key bytes in DBuf; if identical, key found
     - Sentinel `0x0000_0000`: unallocated slot; key is missing
     - Sentinel `0x0000_0001`: tombstone encountered; record first free index for insertion

### 4.4 Bucket Splitting, Atomic Re-hashing & Depth Guard

When an insertion targets a Bucket containing 10 active entries:

1. **Depth Ceiling Guard**:
   Assert `depth < MAX_TRIE_DEPTH` (`0x07`);
   if depth ceiling is reached, fail with `err::COL` (hash collision limit);
   prevents runaway recursive allocation loops on degenerate identical hashes
2. **Contiguous Allocation**:
   Acquire `growth_mutex`; allocate 30 contiguous slots (3840 B) from `free_slot_marker`;
   advance `free_slot_marker += 30`;
   release `growth_mutex`
3. **Pre-Formatting in Private Allocated Space**:
   Format all 30 new slots as empty Buckets (`flag = 0x0000_0001`, `padding = 0`, zeroed hashes);
   rehash the 10 existing entries plus the new 11th entry using `depth + 1` 5-bit folded slice;
   write entries into target child buckets;
   new child slots reside beyond original `free_slot_marker`, invisible to concurrent readers
4. **Atomic OCC Node Publication**:
   Construct new Node contents: `flag = 0x0000_0000`, `padding = 0`, `bucket_indices = new_slots`;
   commit transaction replacing target Bucket slot with Node slot;
   readers transition atomically from reading the old bucket to traversing the new Node;
   guarantees zero partial states or torn reads without write-ahead logging

## 5. Storage Growth & Allocation Protocols

### 5.1 Monotonic Slot Allocation (256-Byte Quantum)

All internal allocations consume uniform integer multiples of 256 B (2 slots);
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

To allocate $K$ slots (where $K$ is an even multiple of 2):
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
Transaction detects capacity exhaustion (DBuf, AdjArr, or Bucket)
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
  Allocates 30 new Buckets (30 slots = 3840 B = 15 x 256 B);
  formats child buckets and converts target slot to Node
- **DBuf Expansion**:
  Allocates 4 new DBuf pools (8 slots = 1024 B = 4 x 256 B = 124 buffers of 8 B);
  zero-initializes bitmaps and buffers;
  appends to DBuf allocation chain via `next_dbuf`
- **AdjArray Expansion**:
  Allocates 2 contiguous AdjArray blocks (2 slots = 256 B = 12 list arrays);
  zero-initializes array bitmaps;
  links blocks together via `next_adjarr_idx`;
  appends to active AdjArray chain originating at `ADJ_ARR_HEAD_SLOT`

### 5.4 Physical OS Page Growth Quantization

When `free_slot_marker + K > total_slots`, backing file expands:

- **64 KiB OS Page Systems**:
  Expand by 1 physical page: `mmap.grow(1)` (64 KiB = 0x0200 slots = 512 slots)
- **16 KiB OS Page Systems**:
  Expand by 1 physical page: `mmap.grow(1)` (16 KiB = 0x80 slots = 128 slots)
- **4 KiB OS Page Systems**:
  Expand by 4 physical pages: `mmap.grow(4)` (16 KiB = 0x80 slots = 128 slots)

Each expansion appends 128 slots (or 512 slots);
`MemMap::grow` validates that `new_length <= MAX_DB_LEN` (`0x80_0000_0000`);
fails with `err::GRW` if the 512 GiB ceiling is breached;
advances `total_pages` and `total_slots` atomically in Slot 0 via `mmap.write(0, ...)`

## 6. Concurrency & Transaction Model (OCC)

Concurrency executes at Slot granularity (128 B) using Optimistic Concurrency Control:

```
Reader:
  1. Read Slot via mmap.read_exact(slot_idx) with torn-read validation
  2. Non-blocking relative to other readers

Writer (Atomic Multi-Slot OCC):
  1. Read required Slots (Bucket hashes, Bucket offsets, DBuf bmap, Payload, AdjArr)
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

## 7. Operational Workflows

### 7.1 `get(key: &[u8]) -> Result<Option<Vec<u8>>>`

1. Compute `hash = hash_key(key)`; initialize `curr_slot = ROOT_NODE_SLOT`, `depth = 0`
2. At `curr_slot`, evaluate `raw = (hash >> (5 * (depth % 6))) & 0x1F`
3. `child_idx = if raw >= 30 { raw - 30 } else { raw }`
4. Read slot at `next_slot = node.bucket_indices[child_idx]`
5. If `flag == 0x0000_0000` (`Node`):
   Set `curr_slot = next_slot`, `depth = depth + 1`, loop to step 2
6. If `flag == 0x0000_0001` (`Bucket`):
   Scan `hashes[0..9]`:
   - If `hashes[i] == hash`:
     Inspect `offsets[i]`:
     if byte 0 bit 7 is set (`offsets[i][0] & 0x80 != 0`):
       return `Err(InvalidType("Key is a list"))`
     Read payload from DBuf buffer `global_buf_idx`;
     validate CRC32 and key equality;
     return `Ok(Some(value_bytes))`
   - If `hashes[i] == 0`:
     Return `Ok(None)`
   Return `Ok(None)` if all 10 slots scanned without match

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
   - If Bucket is full (10 active entries):
     Execute Section 4.4 atomic bucket split;
     allocates 30 child buckets, rehashes entries, publishes new Node, and inserts key

### 7.3 `del(key: &[u8]) -> Result<bool>`

1. Traverse trie to leaf Bucket following Section 4.3;
   if key absent, return `Ok(false)`
2. Atomic OCC commit:
   - Set `hashes[i] = 1` (tombstone)
   - Clear occupied bits in DBuf `bmap`
   - Decrement `Metadata.total_entries`
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
and page validation to `MemMap<Slot>` (`Slot: [u8; 128]`):

#### `create(cfg: TurboCfg) -> TurboResult<TurboFox>`

1. Query host OS page size via `page_size = MemMap::<Slot>::page_size()?`
2. Determine initial allocation pages:
   - `PageSize::K4` (4096 B): `initial_pages = 4` (16 KiB = 0x80 slots)
   - `PageSize::K16` (16384 B): `initial_pages = 1` (16 KiB = 0x80 slots)
   - `PageSize::K64` (65536 B): `initial_pages = 1` (64 KiB = 0x0200 slots)
3. Construct `MemMapCfg` using `cfg.path`, `DB_NAME_TAG`, and `MAX_DB_LEN`
4. Call `mmap = MemMap::<Slot>::new(memmap_cfg)?`:
   - Fails if backing file already exists (`err::EXS`) or directory is invalid (`err::DIR`)
   - Acquires exclusive filesystem advisory lock on file descriptor
   - Reserves virtual address space up to `MAX_DB_LEN`
5. Format initial 6144 B topography (Slots 0..0x2F) directly into mapped memory:
   - Slot 0: Metadata (`magic = 0x54424658`, `version = 3`, `page_size`, `total_pages`,
     `total_slots`, `free_slot_marker = 0x30`, `total_entries = 0`)
   - Slot 1: AdjArr head block (zeroed headers, bitmaps, and `next_adjarr_idx = 47`)
   - Slots 2..15: 7x DBuf pools (zeroed bitmaps, buffers, and next pointers)
   - Slot 16: Root Node (`flag = 0x0`, `padding = 0`, `bucket_indices = [17..46]`)
   - Slots 17..31: 15x Buckets on Page 0 (`flag = 0x1`, zeroed hashes and offsets)
   - Slots 32..46: 15x Buckets on Page 1 (`flag = 0x1`, zeroed hashes and offsets)
   - Slot 47: AdjArr tail block (zeroed headers, bitmaps, `next_adjarr_idx = 0`)
6. Flush initial state via `mmap.sync()?` to guarantee disk persistence
7. Construct and return `TurboFox` engine handle

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
   - Assert `meta.total_slots == (mmap.length() / 128) as u32`; error if corrupt
   - Assert `meta.total_pages == (mmap.length() / sys_page_size.as_usize()) as u32`
   - Assert `meta.free_slot_marker <= meta.total_slots`; error if corrupt
5. Construct and return `TurboFox` engine handle

### 8.2 Engine Configuration & `MemMapCfg` Translation

`TurboCfg` exposes user parameters and translates directly to `MemMapCfg`:

#### Constants & Defaults

```rs
pub const DB_NAME_TAG: [u8; 4] = *b"tbfx";
pub const MODULE_ID: u8 = 0x46;
pub const MAX_DB_LEN: usize = 0x80_0000_0000; // 549,755,813,888 B (512 GiB)
```

#### Configuration Mapping

```
TurboCfg Field  Type             MemMapCfg Target        Resolution Logic
--------------+----------------+-----------------------+--------------------------------------
path          PathBuf          path                    Forwarded directly; MemMap checks dir
version       u32              version                 Compiled constant (LAYOUT_VERSION = 3)
sync_mode     SyncMode         sync_interval           Mapped to Periodic(Duration) or Manual
(constant)    [u8; 4]          name                    DB_NAME_TAG (*b"tbfx")
(constant)    u8               module_id               MODULE_ID (0x46)
(constant)    usize            max_len                 MAX_DB_LEN (0x80_0000_0000)
(computed)    usize            initial_pages           4 on 4 KiB OS; 1 on 16 KiB / 64 KiB
(flag)        bool             memory_priority_hint    true (Windows memory priority advice)
(flag)        bool             sequential_access_hint  true (POSIX_FADV / SEQUENTIAL_SCAN)
```

## 9. Architectural Upper Bounds & Capacity Proof

### 9.1 Addressable Ceilings

The primary architectural ceiling stems from 32-bit slot addressing (`u32`):

- 32 bits address up to $2^{32} = 4{,}294{,}967{,}296$ slots (`0..=0xFFFF_FFFF`)
- At 128 bytes per slot, maximum addressable database file size is 512 GiB (`0x80_0000_0000`):
  $$\text{bytes} = 2^{32} \times 128 = 2^{39} \text{ B} = 549{,}755{,}813{,}888 \text{ B}$$
- Quantized to physical OS pages:
  - 4 KiB pages: $2^{39} / 4096 = 2^{27} = 134{,}217{,}728$ pages (`0x0800_0000`)
  - 16 KiB pages: $2^{39} / 16384 = 2^{25} = 33{,}554{,}432$ pages (`0x0200_0000`)
  - 64 KiB pages: $2^{39} / 65536 = 2^{23} = 8{,}388{,}608$ pages (`0x0080_0000`)

### 9.2 Capacity Bounds for Common Payload Formats

- **DBuf Pools (2 slots / 256 B per DBuf, 31 buffers of 8 B)**:
  - If entire 512 GiB space were dedicated to DBuf pools:
    $$2^{32} / 2 = 2^{31} = 2{,}147{,}483{,}648 \text{ pools}$$
    $$2{,}147{,}483{,}648 \times 31 = 66{,}572{,}000{,}000 \text{ buffers}$$
- **KV Entries (12 B key + 48 B value $\implies$ 64 B frame $\implies$ 8 buffers)**:
  - Each DBuf fits $\lfloor 31 / 8 \rfloor = 3$ entries (7 slack buffers)
- **List Items (20 B value $\implies$ 28 B frame $\implies$ 4 buffers)**:
  - Each DBuf fits $\lfloor 31 / 4 \rfloor = 7$ item payloads (3 slack buffers)
  - Chained list item capacity: bounded by `next_arr_idx: u32` ($2^{32} - 1 \times 4$ items)

### 9.3 Capacity Derivation

$$
\text{Capacity} = 2^{32} \times 128 \text{ B} = 512 \text{ GiB} = 0\text{x}80\_0000\_0000 \text{ B}
$$
