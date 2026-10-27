# Architecture & Technical Specs

TurboFox is a single-file embedded KV storage engine

## 1. Storage Hierarchy & Invariants

```
Unit                 Size      Slots      Alignment  Purpose
-------------------+---------+----------+----------+--------------------------------
Slot (T)           128 B     1          8 B        Base IO / spinlock concurrency
AdjArrayBlock      128 B     1          8 B        1-to-N list index block
Metadata           128 B     1          8 B        Engine configuration and heads
DataBuf            1024 B    8          8 B        Contiguous payload buffer pool
Bucket             1024 B    8          8 B        SoA hash table leaf and overflow
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
  components never shrink or relocate in-place

## 2. File Topography (Initial Footprint)

Initial structure occupies exactly 4096 B (0x20 slots);
backing file maps 16 KiB (0x80 slots) on 4 KiB and 16 KiB systems, or 64 KiB (0x0200 slots) on 64K

```
Slot Range   Byte Range     Size     Component
-----------+--------------+--------+--------------------------------------------------
0            0x0000..0x007F 128 B    Slot 0: Metadata
1..7         0x0080..0x03FF 896 B    7x Adjacent Array blocks (128 B each)
8..15        0x0400..0x07FF 1024 B   Root Bucket (8 slots, SoA table)
16..31       0x0800..0x0FFF 2048 B   2x DataBuf pools (1024 B each, 8 slots each)
32..N        0x1000..EOF    Rest     Unallocated slots managed by free_slot_marker
```

- Initial Index: 1 Root Bucket (Slots 8..0x0F, constant `ROOT_BUCKET_SLOT = 8`)
- Initial Payloads: 2 DataBufs = 126 buffers of 16 B (head `DATABUF_HEAD_SLOT = 16`)
- Initial Lists: 7 AdjArray blocks = 42 list arrays (head `ADJ_ARR_HEAD_SLOT = 1`)
- Unallocated: Slots 0x20..0x7F (16 KiB mapping) or 0x20..0x01FF (64 KiB mapping)

## 3. Component Memory Layouts

### 3.1 Static Topography Constants & Metadata (Slot 0, 128 Bytes, `align(8)`)

#### Engine Static Head Constants

Base component head slot indices remain constant across the database lifetime;
hardcoded as immutable compile-time constants rather than stored in Metadata:

```rs
pub const ROOT_BUCKET_SLOT: u32 = 0x08;  // Slot 8 (Byte offset 0x0400)
pub const ADJ_ARR_HEAD_SLOT: u32 = 0x01; // Slot 1 (Byte offset 0x0080)
pub const DATABUF_HEAD_SLOT: u32 = 0x10; // Slot 16 (Byte offset 0x0800)
```

#### Metadata Layout

```
Offset   Type      Field             Description
-------+---------+-----------------+--------------------------------------------------
0..3     [u8; 4]   magic             0x54424658 ("TBFX")
4..7     u32       version           Layout version (2)
8..11    u32       page_size         Host OS page size (4096, 16384, or 65536)
12..15   u32       total_pages       Total allocated physical OS pages
16..19   u32       total_slots       Total addressable slots in mapped file
20..23   u32       free_slot_marker  Next unallocated slot index (starts at 32 / 0x20)
24..31   u64       total_entries     Active key count (KV + List)
32..127  [u8; 96]  reserved          Zero-padded reserved space
```

### 3.2 Bucket (1024 Bytes, 8 Slots, `align(8)`)

Structure of Arrays (SoA) layout spanning exactly 8 slots (0x400 B);
contains 64 entries partitioned into parallel arrays for hash matching, offsets, and links

```
Slot Offset  Byte Range     Type        Field                Description
-----------+--------------+-----------+--------------------+--------------------------
0..1         0x0000..0x00FF [u32; 64]   hashes               Normalized key hashes
2..5         0x0100..0x02FF [u64; 64]   offsets              Packed entry metadata
6..7         0x0300..0x03FF [u32; 64]   next_bucket_indices  Chained overflow buckets
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

Reading the first 4 bytes as Big-Endian `u32::from_be_bytes` or inspecting Byte 0
(`bytes[0] & 0x80`) evaluates the identical bit across all architectures;
preventing layout divergence between `u8` and `u32` views:

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
- `Bytes 4..7 (global_buf_idx)`: Global 16 B DataBuf buffer index in little-endian

**List Entry Format (8 Bytes)**:
```
Bytes 0..3 (Big-Endian u32)                    Bytes 4..7 (LE)
+--------------------------------------------+------------------------+
| Flag (Bit 31 = 1) | adjarr_idx (Bits 0..30) | global_buf_idx (u32)  |
+--------------------------------------------+------------------------+
```
- `Bytes 0..3 (flag | adjarr_idx)`:
  Packed as Big-Endian `u32` (Bit 31 = 1, Bits 0..30 = relative array index);
  evaluates identically as `(bytes[0] & 0x80) != 0` or `from_be_bytes(b) & 0x8000_0000 != 0`
- `Bytes 4..7 (global_buf_idx)`: Global 16 B DataBuf buffer index holding key framing

#### Next Bucket Index (`next_bucket_indices[i]`, 4 Bytes)

- `0`: No chained bucket (unallocated sentinel)
- `> 0`: Absolute slot index of chained 8-slot overflow Bucket

### 3.3 DataBuf (1024 Bytes, 8 Slots, `align(8)`)

Pool of 63 16-byte buffers for raw key and value payloads;
occupies exactly 8 slots (0x400 B)

```
Offset      Type            Field      Description
----------+---------------+----------+------------------------------------------------
0..1007     [[u8; 16]; 63]  buffers    63 payload buffers (16 B each = 1008 B)
1008..1015  u64             bitmap     Bits 1..63: buffer occupancy; bit 0: pool full
1016..1019  u32             next_dbuf  Slot index of next DataBuf in chain (0 = tail)
1020..1023  [u8; 4]         padding    Zero alignment padding to 1024 B
```

#### Payload Framing

Contiguous buffers allocate blocks according to total framed length:

- **KV Framing**:
  Allocates $\lceil (4 + \text{klen} + \text{vlen}) / 16 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ Key Bytes (klen) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```
- **List Key Framing**:
  Allocates $\lceil (5 + \text{klen}) / 16 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ klen (1 B) ][ Key Bytes (klen) ][ 0-Padding ]
  ```
- **List Item Value Framing**:
  Allocates $\lceil (6 + \text{vlen}) / 16 \rceil$ contiguous buffers
  ```
  [ CRC32 (4 B, LE) ][ vlen (2 B, LE) ][ Value Bytes (vlen) ][ 0-Padding ]
  ```

#### Two-Phase Read Process (List Key and List Item Value)

List keys and list values store explicit length headers in their head buffer;
enables exact multi-buffer traversal without reading redundant trailing buffers:

- **List Key Read**:
  1. Read single 16 B buffer at `dbuf_idx`; contains `[ CRC32 (4 B) ][ klen (1 B) ][ Key[0..10] ]`
  2. Inspect `klen` (Byte 4); if `klen <= 11`, key payload resides entirely within initial buffer
  3. If `klen > 11`, compute remaining buffers: $N = \lceil (\text{klen} - 11) / 16 \rceil$;
     read exactly $N$ contiguous buffers starting at `dbuf_idx + 1`
  4. Concatenate bytes and verify CRC32

- **List Item Value Read**:
  1. Read single 16 B buffer at `dbuf_idx`; contains `[ CRC32 (4 B) ][ vlen (2 B) ][ Val[0..9] ]`
  2. Inspect `vlen` (Bytes 4..5, LE); if `vlen <= 10`, value resides entirely within initial buffer
  3. If `vlen > 10`, compute remaining buffers: $N = \lceil (\text{vlen} - 10) / 16 \rceil$;
     read exactly $N$ contiguous buffers starting at `dbuf_idx + 1`
  4. Concatenate bytes and verify CRC32

Global buffer address translation;
for global buffer index $B$:
$$\text{dbuf\\_slot} = \text{DATABUF\\_HEAD\\_SLOT} + \dots$$
$$\text{local\\_buf} = B \pmod{63}$$

### 3.4 Adjacent Array Block (128 Bytes, 1 Slot, `align(8)`)

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

- `dbuf_idx: u32` (4 B): Global DataBuf buffer index holding `[CRC32][vlen][Value]`

#### `AdjArrayList` (20 Bytes, `repr(C)`)

- `entries: [AdjArrayEntry; 4]` (16 B): 4 list item pointers
- `next_arr_idx: u32` (4 B): Global relative index of next array in chain (`0` = tail)

#### Global Relative Array Indexing

For relative array index $A$:
$$\text{block\\_hop} = \lfloor A / 6 \rfloor$$
$$\text{local\\_array} = A \pmod{6}$$

Traverse `next_adjarr_idx` block chain by `block_hop` steps;
index into `arrays[local_array]`

## 4. Hash Routing, Probing & Overflow Chaining

### 4.1 Key Hash Normalization

Algorithm: `twox_hash::XxHash32` initialized with seed `0x5442_4658` (`"TBFX"`)

```rs
let h = hasher.finish() as u32;
let hash = if h < 2 { h + 2 } else { h };
```

- `0`: Empty slot sentinel
- `1`: Tombstone (deleted) sentinel
- `>= 2`: Valid normalized key hash

### 4.2 Hash Bit-Slicing

32-bit hash routes through arbitrary depth levels via cyclic 6-bit slices;
a 32-bit key hash yields 5 full 6-bit slices (bits 0..29);
traversal past depth 4 resets cyclically to the initial 6 bits (`depth % 5`):

```
Depth 0: (hash >> 0)  & 0x3F (0..63) -> Slice 0: bits 0..5
Depth 1: (hash >> 6)  & 0x3F (0..63) -> Slice 1: bits 6..11
Depth 2: (hash >> 12) & 0x3F (0..63) -> Slice 2: bits 12..17
Depth 3: (hash >> 18) & 0x3F (0..63) -> Slice 3: bits 18..23
Depth 4: (hash >> 24) & 0x3F (0..63) -> Slice 4: bits 24..29
Depth 5: (hash >> 0)  & 0x3F (0..63) -> Slice 0: reset to bits 0..5
Depth D: (hash >> (6 * (D % 5))) & 0x3F -> Slice (D % 5)
```

### 4.3 Open Addressing & Overflow Chaining

Every lookup, insert, and delete follows identical traversal rules;
no rehashing occurs; buckets chain forever

1. Compute primary slot index for current depth $D$:
   $$S = \left(\text{hash} \gg (6 \cdot (D \pmod{5}))\right) \mathbin{\\&} 0\text{x}3\text{F}$$
2. Probe up to 16 consecutive slots using SIMD vector acceleration:
   $$P_i = (S + i) \pmod{64} \quad \text{for } i \in [0, 15]$$
   - 16 candidate `u32` hashes span exactly 64 bytes (0x40 B, 1 cache line);
     scanned concurrently without scalar branch penalties
   - If `hashes[P_i] == hash`:
     Compare key bytes stored in DataBuf;
     if key matches, entry found (target slot is $P_i$)
   - If `hashes[P_i] == 0`:
     Key missing;
     insertion selects first encountered tombstone or slot $P_i$
   - If `hashes[P_i] == 1`:
     Tombstone encountered;
     record first tombstone candidate and continue probe
3. If all 16 open-addressing slots are occupied without match or empty slot:
   - Read `next_slot = next_bucket_indices[S]` attached to primary index $S$
   - If `next_slot == 0`:
     - Insertion: allocate new 8-slot Bucket;
       link `next_bucket_indices[S] = new_bucket_slot`;
       advance $D \leftarrow D + 1$ and repeat at step 1 in new Bucket
     - Lookup / Delete: key is absent;
       terminate and return not found
   - If `next_slot > 0`:
     Advance $D \leftarrow D + 1$ and repeat at step 1 in Bucket at `next_slot`

### 4.4 SIMD Probe Acceleration & Runtime ISA Dispatch

Open addressing evaluates up to 16 (0x10) candidate slots (64 B, single cache line) concurrently
using vectorized comparisons selected at runtime:

- **ISA Detection via `frozen_core::isa`**:
  Host CPU features are detected at runtime via `frozen_core::isa`;
  dispatches lookup, insert, and delete probes to specialized vector kernels:
  - `open_address_avx2`: x86_64 AVX2 256-bit SIMD path
  - `open_address_sse2`: x86_64 SSE2 128-bit SIMD path
  - `open_address_neon`: AArch64 NEON 128-bit vector path
  - `open_address_scalar`: Portable scalar fallback path
- **AVX2 Implementation (`open_address_avx2`)**:
  Processes 16 `u32` hashes via two 256-bit `_mm256_cmpeq_epi32` comparisons;
  extracts bitmasks with `_mm256_movemask_ps` for parallel match extraction
- **SSE2 Implementation (`open_address_sse2`)**:
  Processes 16 `u32` hashes via four 128-bit `_mm_cmpeq_epi32` comparisons;
  extracts bitmasks with `_mm_movemask_ps`
- **NEON Implementation (`open_address_neon`)**:
  Processes 16 `u32` hashes via four 128-bit `vceqq_u32` vector comparisons;
  narrows match masks to resolve slot hits across candidate lanes
- **Index Resolution**:
  Bitmasks are resolved using count trailing zeros (`ctz` / `trailing_zeros`);
  identifies target slot, first empty sentinel (`0x00`), or first tombstone (`0x01`)

### 4.5 Optional In-Memory Fingerprint Index (`FpIndex`)

#### 4.5.1 Architecture & Objectives

To bypass persistent, memory-mapped bucket-chain traversal on hot read/write paths,
TurboFox provides an optional SIMD-accelerated in-memory index (`FpIndex`), enabled via
`TurboCfg::with_mem_index(true)`:

- **32-Way Concurrent Sharding**:
  Hash space partitioned across 32 independent shards (`FpIndexShard`), each protected
  by `std::sync::RwLock`; allows non-blocking concurrent reads across multi-core workloads
- **Zero Bucket Reads on Hit**:
  Caches persistent slot locations and DataBuf routing directly in RAM;
  KV read hits read directly from the resolved physical DataBuf page in mmap,
  bypassing 1024 B `Bucket` read copies and linked-list pointer chasing
- **Negative Rejection (Definite Miss)**:
  Non-existent keys resolve during in-memory probe upon encountering empty sentinels (`0x0000`),
  executing 0 mmap reads and avoiding backing file lookups
- **Fast-Path Mutations**:
  Overwrites and absent deletions resolve immediately through `FpIndex` routing,
  avoiding variable-depth persistent hash-trie traversal

#### 4.5.2 `IndexRef` Memory Layout (24 Bytes, `align(8)`)

```
Offset   Type      Field        Description
-------+---------+------------+--------------------------------------------------
0..3     u32       hash         Full 32-bit normalized key hash
4..7     u32       bucket_slot  Persistent bucket slot index for OCC commit
8..11    u32       dbuf_slot    Physical DataBuf slot index (bypasses pointer chasing)
12       u8        entry_idx    Bucket entry index (0..63)
13..15   [u8; 3]   _pad         Zero alignment padding to 8 B
16..23   u64       offset       Packed entry metadata (flags, klen, vlen, buf_idx)
```

- Direct discriminator check: `(offset & 0x80) != 0` identifies list heads directly from metadata
- Length resolution: `klen` (Byte 1) and `vlen` (Bytes 2..3) decoded via bit shifts from `offset`

#### 4.5.3 16-Bit Fingerprints & SIMD Probing

Each shard stores parallel arrays of 16-bit fingerprints (`fingerprints: Vec<u16>`)
and entries (`refs: Vec<IndexRef>`):

- **Fingerprint Calculation**:
  Upper 16 bits of 32-bit key hash (`hash >> 16`), mapped to valid range (`>= 2`)
- **Sentinels**:
  `0x0000` (Empty slot), `0x0001` (Tombstone)
- **16-Probe Window**:
  Probes 16 consecutive fingerprint slots (32 bytes = half cache line)
- **Vector Kernels (`index_probe.rs`)**:
  - AVX2: 256-bit `_mm256_cmpeq_epi16` + `_mm256_packs_epi16` + `_mm256_movemask_epi8`
  - SSE2: Two 128-bit `_mm_cmpeq_epi16` + `_mm_packs_epi16` + `_mm_movemask_epi8`
  - NEON: Two 128-bit `vceqq_u16` with bit shifts and narrowing
  - Scalar: 16-iteration loop fallback
- **Bitmask Extraction (`FpProbeMask`)**:
  Simultaneously evaluates `match_mask`, `empty_mask`, and `tomb_mask`;
  if `empty_mask` reveals an empty slot prior to match, terminates with `LookupResult::DefiniteMiss`

#### 4.5.4 Shard Resizing & Pre-Population

- Shards dynamically double capacity when load factor reaches 70%:
  `(count + tombstones) * 10 >= cap * 7`
- Pre-population during `open()` executes a BFS traversal across persistent Bucket chains;
  pre-caches databuf hop-to-slot indices in $O(B)$ for $B$ chained buckets, enabling $O(1)$ routing
  per entry without redundant mmap linked-list traversals

## 5. Storage Growth & Allocation Protocols

### 5.1 Monotonic Slot Allocation (Bump Pointer)

All internal allocations consume uniform 8-slot (1024 B) blocks;
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

To allocate 8 contiguous slots:
1. Acquire engine `growth_mutex`
2. Assert `free_slot_marker + 8 <= total_slots`;
   if capacity exceeded, invoke physical growth procedure first
3. `allocated_slot = free_slot_marker`
4. `free_slot_marker += 8`
5. Release `growth_mutex`
6. Return `allocated_slot`

### 5.2 Multi-Tx Growth Coordination (Lock-and-Recheck)

Concurrent writers handle exhaustion via Double-Checked Locking;
prevents allocation stampedes and duplicate file expansion:

```
Transaction detects capacity exhaustion (DataBuf, AdjArr, or Bucket)
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

### 5.3 Component Expansions (8-Slot Uniform Units)

Every internal expansion strictly allocates 8 slots (1024 B):

- **Bucket Expansion**:
  Allocates 1 new Bucket (8 slots = 1024 B);
  zero-initialized (all hashes = 0, next pointers = 0);
  linked from parent bucket `next_bucket_indices[S]`

- **DataBuf Expansion**:
  Allocates 1 new DataBuf (8 slots = 1024 B);
  zero-initializes bitmap and buffers;
  appends to DataBuf allocation chain via `next_dbuf`

- **AdjArray Expansion**:
  Allocates 8 contiguous AdjArray blocks (8 x 128 B = 1024 B = 8 slots = 48 arrays);
  zero-initializes array bitmaps;
  links blocks together via `next_adjarr_idx`;
  links into active AdjArray block chain originating at `ADJ_ARR_HEAD_SLOT`

### 5.4 Physical OS Page Growth Quantization

When `free_slot_marker + 8 > total_slots`, backing file expands:

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
  1. Read Slot via mmap.read_exact(slot_idx) with torn-read protection
  2. Non-blocking relative to other readers

Writer (Atomic Multi-Slot OCC):
  1. Read required Slots (Bucket hashes, Bucket offsets, DataBuf bitmap, Payload, AdjArr)
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

When `cfg.mem_index` is enabled, lookups probe `FpIndex` before touching mmap:
1. Probe `fp_index.lookup(hash, verify)`:
   - On `DefiniteMiss`: key is absent; return `Ok(None)` immediately with 0 mmap reads
   - On `Hit(r)`: verify list discriminator bit (`r.offset & 0x80`);
     if set, return `Err(TurboError::InvalidType("Key is a list"))`;
     read payload directly from physical page `r.dbuf_slot`, validating key equality;
     bypasses persistent `Bucket` read copies and linked-list traversal
   - On `Overflow`: fall back to persistent bucket walk below

Persistent Bucket Traversal:
1. Compute `hash = hash_key(key)`; initialize `bucket_slot = ROOT_BUCKET_SLOT`, `depth = 0`
2. Compute `S = (hash >> (6 * (depth % 5))) & 0x3F`
3. Probe up to 16 slots $P_i = (S + i) \pmod{64}$ for $i \in [0, 15]$ via ISA SIMD kernel:
   - If `hashes[P_i] == hash`:
     - Inspect `offsets[P_i]`:
       if byte 0 bit 7 is set (`offsets[P_i][0] & 0x80 != 0`):
         return `Err(InvalidType("Key is a list"))`
     - Read payload from DataBuf buffer `global_buf_idx`
     - Validate CRC32 and key equality;
       return `Ok(Some(value_bytes))`
   - If `hashes[P_i] == 0`:
     Return `Ok(None)`
4. Follow `next_bucket_indices[S]`;
   if 0, return `Ok(None)`;
   else set `bucket_slot = next_bucket_indices[S]`, `depth = depth + 1`, loop to step 2

### 7.2 `set(key: &[u8], val: &[u8]) -> Result<TurboTicket>`

1. Validate `key.len() <= 255` and `val.len() <= 65535`
2. If `cfg.mem_index` is enabled, probe `fp_index`:
   if key exists, invoke fast-path `set_overwrite` directly targeting resolved bucket and databuf
   slots without depth traversal; commit via OCC transaction and update `FpIndex`
3. For new insertions, traverse to target Bucket and slot following Section 4.3
4. If entry exists and `new_len <= old_allocated`:
   Overwrite value in-place via OCC tx
5. If insert or payload expansion:
   - If 16 open-addressing slots full and `next_bucket_indices[S] == 0`:
     Allocate new Bucket from `free_slot_marker`, link to parent
   - Allocate contiguous 16 B buffers in active DataBuf chain
   - Build payload frame `[ CRC32 ][ Key ][ Val ][ Pad ]`
   - Atomic OCC commit:
     Update DataBuf bitmap, DataBuf buffer slots, Bucket hash, Bucket offset;
     if `cfg.mem_index` enabled, insert/update entry in `fp_index`

### 7.3 `del(key: &[u8]) -> Result<bool>`

1. If `cfg.mem_index` is enabled, query `fp_index`:
   - If `DefiniteMiss`: key absent; return `Ok(false)` with 0 mmap reads
   - If `Hit(r)`: invoke fast-path `del_target` directly targeting `(r.bucket_slot, r.entry_idx)`
     and `r.dbuf_slot`; commit tombstone via OCC transaction, remove entry from `fp_index`,
     and return `Ok(true)`
2. For persistent traversal fallback, locate Bucket entry slot $P_i$;
   if missing, return `Ok(false)`
3. Atomic OCC commit:
   - Set `hashes[P_i] = 1` (tombstone)
   - Preserve `next_bucket_indices` intact to maintain overflow connectivity
   - Clear occupied bits in DataBuf bitmap
   - Decrement `Metadata.total_entries`
   - If `cfg.mem_index` enabled, remove entry from `fp_index`
4. Return `Ok(true)`

### 7.4 List Operations (`TurboList`)

- Key registered in Bucket with byte 0 bit 7 set to 1 (`offsets[i][0] & 0x80 != 0`);
  stores `adjarr_idx` (relative array index) and `global_buf_idx` (key payload)
- DataBuf stores `[ CRC32 ][ klen (1 B) ][ Key ][ Pad ]`
- Values stored across chained `AdjArrayList` nodes (4 items per 20 B array)
- `append`:
  Locate tail array;
  if all 4 entries occupied, allocate new array from AdjArr block;
  link `prev.next_arr_idx = new_arr_idx`
- `pop`:
  Read tail item via two-phase read, clear DataBuf buffers, decrement array count;
  if array becomes empty, return to block bitmap
- `first` / `last` / `get_at(idx)` / `iter`:
  Traverse array chains;
  load item values via two-phase DataBuf buffer reads, validating CRC32 per item

## 8. Lifecycle & Versioning

### 8.1 Database Instantiation Workflows

Initialization delegates low-level OS mapping, file creation, descriptor locking,
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
5. Format initial 4 KiB topography (Slots 0..0x1F) directly into mapped memory:
   - Slot 0: Metadata (`magic = 0x54424658`, `version`, `page_size`, `total_pages`,
     `total_slots`, `free_slot_marker = 0x20`, `total_entries = 0`)
   - Slots 1..7: 7x AdjArr blocks (zeroed headers, bitmaps, and next pointers)
   - Slots 8..0x0F: Root Bucket (zeroed hashes, offsets, and chained bucket pointers)
   - Slots 0x10..0x1F: 2x DataBuf pools (zeroed bitmaps, buffers, and next pointers)
6. Flush initial state via `mmap.sync()?` to guarantee disk persistence
7. If `cfg.mem_index` is enabled, initialize in-memory `FpIndex` with base capacity `MIN_CAPACITY`
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
   - Assert `meta.total_slots == (mmap.length() / 128) as u32`; error if corrupt
   - Assert `meta.total_pages == (mmap.length() / sys_page_size.as_usize()) as u32`
   - Assert `meta.free_slot_marker <= meta.total_slots`; error if corrupt
5. If `cfg.mem_index` is enabled, allocate `FpIndex` sized to expected entries;
   execute BFS traversal across persistent Bucket chains to pre-populate index routing
6. Construct and return `TurboFox` engine handle

### 8.2 Engine Configuration & `MemMapCfg` Translation

`TurboCfg` exposes user parameters and translates directly to `MemMapCfg`;
TurboFox delegates all directory presence, path validation, and file locking to `MemMap`:

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
version       u32              version                 Compiled constant (LAYOUT_VERSION = 2)
sync_mode     SyncMode         sync_interval           Mapped to Periodic(Duration) or Manual
mem_index     bool             (internal engine)       Enables optional in-memory FpIndex
(constant)    [u8; 4]          name                    DB_NAME_TAG (*b"tbfx")
(constant)    u8               module_id               MODULE_ID (0x46)
(constant)    usize            max_len                 MAX_DB_LEN (0x80_0000_0000)
(computed)    usize            initial_pages           4 on 4 KiB OS; 1 on 16 KiB / 64 KiB
(flag)        bool             memory_priority_hint    true (Windows memory priority advice)
(flag)        bool             sequential_access_hint  true (POSIX_FADV / SEQUENTIAL_SCAN)
```

#### Backing File Path Resolution

The backing file path is managed by `MemMapCfg::versioned_path()`;
formatted as `{path}/{name}_{version}` without file extension:

- `path` must resolve to an existing directory (validated by `MemMap`)
- Separate schema versions co-exist without naming collisions;
  e.g. `tbfx_1` and `tbfx_2` reside independently within the same directory path

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

- **DataBuf Pools (8 slots / 1024 B per DataBuf, 63 buffers of 16 B)**:
  - If entire 512 GiB space were dedicated to DataBuf pools:
    $$2^{32} / 8 = 2^{29} = 536{,}870{,}912 \text{ pools}$$
    $$536{,}870{,}912 \times 63 = 33{,}822{,}867{,}456 \text{ buffers}$$
- **KV Entries (12 B key + 48 B value $\implies$ 64 B frame $\implies$ 4 buffers)**:
  - Each DataBuf fits $\lfloor 63 / 4 \rfloor = 15$ entries (3 slack buffers)
- **List Items (42 B value $\implies$ 48 B frame $\implies$ 3 buffers)**:
  - Each DataBuf fits $63 / 3 = 21$ item payloads (0 slack buffers)
  - Chained list item capacity: bounded by `next_arr_idx: u32` ($2^{32} - 1 \times 4$ items)

### 9.3 Capacity Derivation

$$\text{Capacity} = 2^{32} \text{ slots} \times 128 \text{ B} = 549{,}755{,}813{,}888 \text{ B} = 512 \text{ GiB} = 0\text{x}80\_0000\_0000 \text{ B}$$
