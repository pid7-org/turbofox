# Architecture & Technical Specs

TurboFox is a single file embedded KV storage engine

## 1. Storage Hierarchy & Invariants

```
Unit                 Size      Slots (T)  VP Factor  Alignment  Purpose
-------------------+---------+----------+----------+----------+--------------------------------
Slot (T)           128 B     1          1/2 VP     8 B        Base IO / spinlock concurrency
Virtual Page (VP)  256 B     2          1.0 VP     8 B        Unit for Nodes and Buckets
DataBuf            1024 B    8          4.0 VP     8 B        Contiguous payload buffer pool
AdjArrayBlock      128 B     1          1/2 VP     8 B        List pointer allocation block
Physical OS Page   4096 B    32         16.0 VP    4096 B     Host filesystem growth unit
```

Following invariants must be preserved,

- All disk structures are strictly `#[repr(C)]`, integer multiples of `Slot` (128 B), and 8B aligned
- File growth is strictly quantized to physical OS pages (default 4096 B = 32 slots)
- Concurrency uses Optimistic Concurrency Control (OCC) with per-slot reader spinlocks;
  ordered multi-slot CAS commit guarantees deadlock-free transactions

## 2. File Topography (Initial 32 KiB Allocation)

Database initializes at 8 physical pages (32,768 bytes, 256 slots, 128 Virtual Pages)

```
Page  Slots     VP        Layout
----+---------+---------+-------------------------------------------------------------
1     0..31     0..15     Slot 0: Metadata (128 B)
                          Slots 1..5: 5x Adjacent Array blocks (640 B)
                          Slots 6..7: Root Node (256 B, VP 3)
                          Slots 8..31: 12x Buckets (3072 B, VP 4..15)
2     32..63    16..31    16x Buckets (4096 B, VP 16..31)
3     64..95    32..47    16x Buckets (4096 B, VP 32..47)
4     96..127   48..63    16x Buckets (4096 B, VP 48..63)
5     128..159  64..79    Slots 128..135: 4x Buckets (1024 B, VP 64..67) -> 64 total
                          Slots 136..159: 3x DataBufs (3072 B, VP 68..79)
6     160..191  80..95    4x DataBufs (4096 B, VP 80..95)
7     192..223  96..111   Slots 192..199: 8x Adjacent Array blocks (1024 B, VP 96..99)
                          Slots 200..223: 3x DataBufs (3072 B, VP 100..111)
8     224..255  112..127  4x DataBufs (4096 B, VP 112..127)
```

- Index: 1 Root Node (VP 3) + 64 Buckets (VP 4..67)
- Payloads: 14 DataBufs = 882 buffers of 16 B
- Lists: 13 AdjArray blocks = 78 arrays of 20 B

## 3. Component Memory Layouts

### 3.1 Metadata (Slot 0, 128 Bytes, `align(8)`)

```
Offset  Type     Field          Description
------+--------+--------------+------------------------------------------------
0..3    [u8; 4]  magic          0x54424658 ("TBFX")
4..7    u32      version        Layout version (1)
8..11   u32      page_size      Host OS page size (4096)
12..15  u32      root_node_vp   Virtual page index of root Node (3)
16..19  u32      total_pages    Total allocated physical pages (8)
20..23  u32      total_slots    Total allocated slots (256)
24..31  u64      total_entries  Active key count (KV + List)
32..35  u32      adj_arr_head   Head block index of AdjArray free-list
36..39  u32      databuf_head   Head index of DataBuf allocation chain
40..127 [u8; 88] reserved       Zero-padded reserved space
```

### 3.2 Virtual Page Discrimination (Offset 0..3)

Both Node and Bucket occupy exactly 256 bytes (2 slots)

Discriminator check on 32-bit header at byte offset 0,

- `header & 0x8000_0000 == 0`: **Node**
- `header & 0x8000_0000 != 0`: **Bucket**

### 3.3 Node (256 Bytes, `align(8)`)

64-way Trie branch indexing child Nodes or Buckets

```
Offset  Type       Field       Description
------+----------+-----------+---------------------------------------------------
0..255  [u32; 64]  children    Target child VP index (bits 0..30); bit 31 must be 0
                               Value 0 = unallocated / empty branch
```

### 3.4 Bucket & BucketEntry (256 Bytes, `align(8)`)

Hash table leaf containing 21 entries probed via open addressing

```
Offset  Type                 Field    Description
------+--------------------+--------+------------------------------------------
0..3    u32                  header   Bit 31 = 1 (Bucket marker); bits 0..30 flags
4..255  [BucketEntry; 21]    entries  21 x 12 B = 252 B
```

`BucketEntry` (12 Bytes, `repr(C)`),

```
Offset  Type  Field    Description
------+-----+--------+----------------------------------------------------------
0..1    u16   klen     Key length in bytes
2..3    u16   vlen     KV: Value byte length; List: Head AdjArray index
4..7    u32   buf_idx  Bit 31: 0 = KV, 1 = List; bits 0..30: DataBuf buffer index
8..11   u32   hash     0 = Empty, 1 = Tombstone, >=2 = Normalized XxHash32
```

### 3.5 DataBuf (1024 Bytes, 4 VP / 8 Slots, `align(8)`)

Pool of 63 16-byte buffers for raw key and value payload storage

```
Offset     Type            Field      Description
---------+---------------+----------+------------------------------------------
0..1007    [[u8; 16]; 63]  buffers    63 payload buffers
1008..1015 u64             bitmap     Bits 0..62: 1=occupied, 0=free; bit 63: full
1016..1019 u32             next_dbuf  Bits 0..30: Next DataBuf index in chain
1020..1023 [u8; 4]         padding    Alignment padding to 1024 B
```
Payload Framing,

Contiguous buffers allocate `ceil((4 + klen + vlen) / 16)` slots,

```
[ CRC32 (4 B, Little-Endian) ][ Key Bytes (klen) ][ Value Bytes (vlen) ][ 0-Padding ]
```

For list keys, DataBuf stores key only (`ceil((4 + klen) / 16)`)

List item values store `[ CRC32 ][ Value ]` independently in DataBuf

### 3.6 Adjacent Array Block (128 Bytes, 1 Slot, `align(8)`)

Index block for 1-to-N list structures

```
Offset    Type                 Field    Description
--------+--------------------+--------+----------------------------------------
0..3      u32                  header   Bit 31: 1=occupied, 0=free; bits 0..30 next
4..123    [AdjArrayList; 6]    arrays   6 x 20 B list arrays = 120 B
124..125  u16                  bitmap   Bits 0..5: Array occupancy
126..127  [u8; 2]              padding  Alignment padding to 128 B
```

`AdjArrayEntry` (6 Bytes, `repr(C, packed(2))`),

- `buf_idx: u32` (4 B): DataBuf buffer index holding `[CRC32][Value]`
- `vlen: u16` (2 B): Value byte length


`AdjArrayList` (20 Bytes, `repr(C, packed(2))`),

- `entries: [AdjArrayEntry; 3]` (18 B): 3 list item pointers
- `next_arr: u16` (2 B): Global index of next array in chain (`0` = tail)
- Global index `A`:
  `block = A / 6`;
  `array = A % 6`;
  `byte_offset = 4 + (array * 20)`

## 4. Hash Routing & Probing

### 4.1 Key Hash Normalization

Algorithm: `twox_hash::XxHash32` with seed `0x5442_4658` (`"TBFX"`)

```rs
let h = hasher.finish() as u32;
let hash = if h < 2 { h + 2 } else { h };
```

- `0`: Empty slot sentinel
- `1`: Tombstone (deleted) sentinel
- `>= 2`: Valid normalized key hash

### 4.2 Trie Bit-Slicing

32-bit hash routes through Trie depth $D \in [0, 4]$ via 6-bit slices:

```
Depth 0: hash[0..5]   (0..63) -> Root Node child index
Depth 1: hash[6..11]  (0..63) -> Child Node child index
Depth 2: hash[12..17] (0..63) -> Child Node child index
Depth 3: hash[18..23] (0..63) -> Child Node child index
Depth 4: hash[24..29] (0..63) -> Child Node child index
```

### 4.3 Open Addressing Probing

Initial slot in Bucket at depth $D$:
$$\text{slot}_0 = \left(\text{hash} \gg (6 \cdot D)\right) \pmod{21}$$

Probe linearly across $(\text{slot}_0 + i) \pmod{21}$ for $i \in [0, 20]$,

1. `entry.hash == target_hash`:
   Compare key bytes in DataBuf;
   if matched, key found
2. `entry.hash == 0`:
   Key missing;
   insert into first tombstone found or this slot
3. `entry.hash == 1`:
   Tombstone encountered;
   record slot and continue probe

## 5. Storage Growth Protocols

All allocations execute through `MemMap::grow(pages)` (quantized to physical OS pages)

Readers and writers stall briefly during remap/grow via `MemMap` internal `RwLock` write guards

### 5.1 Multi-Tx Growth Coordination (Lock-and-Recheck)

In concurrent multi-transaction environments, uncoordinated growth calls can lead to allocation
stampedes and duplicate file growth. TurboFox enforces an engine-level **Double-Checked Locking
(Lock-and-Recheck)** protocol,

```
Transaction detects capacity exhaustion (DataBuf, AdjArray, or Bucket)
  │
  ▼
Acquire TurboFox engine-level `growth_mutex`
  │
  ▼
[RE-CHECK] Has capacity already been expanded by a preceding transaction?
  ├── YES (Resource available) ──► Release `growth_mutex` ──► Proceed with allocation
  │
  └── NO  (Resource still exhausted)
        │
        ├─► Call `MemMap::grow(pages)` (safe remap; halts concurrent read/write during remap)
        ├─► Format new components in-place (bitmaps, headers, sentinels)
        ├─► Update engine Metadata (Slot 0: `total_pages`, `total_slots`, freelist heads)
        ├─► Release `growth_mutex`
        └─► Proceed with transaction / publish changes
```

Following invariants must be satisfied,

- **Stampede Elimination**: Queued transactions waiting on `growth_mutex` do not blindly re-grow;
    When the lock is granted, the transaction re-evaluates capacity; if a prior transaction already
    expanded the relevant freelist or split the target bucket, `mmap.grow()` is bypassed
    
- **Independent Trigger Support**: Because exhaustion may stem from distinct triggers (e.g. Tx 1
    needing DataBuf while Tx 2 needs an AdjArray or a different Bucket split), the re-check
    accurately identifies whether that specific transaction's resource is still missing, permitting
    subsequent legitimate growth
    
- **Zero-Stall Component Initialization**: Newly grown slots reside beyond the active database
    boundary and are completely unreferenced; Formatting newly allocated Buckets, DataBufs, or
    AdjArrays occurs concurrently without stalling other readers or writers
    
- **Atomic Split Publishing (RCU / OCC)**: When splitting a Bucket, child Buckets are initialized in
    newly grown space first; The split is published atomically by converting the old Bucket into a
    Node (`header &= 0x7FFF_FFFF`) via `MemMapTransaction` (staged across 2 slots); Concurrent
    readers observe either the intact old Bucket or traverse the new Node, preventing torn states

### 5.2 Bucket Split (Trie Deepening)

- **Trigger**:
  Insert attempts into a 21-entry Bucket with 0 empty or tombstone slots

- **Allocation**:
  Grow by 4 OS pages (16,384 B = 128 slots = 64 Buckets)

- **Transformation**:
  1. Invert old Bucket header: write `header &= 0x7FFF_FFFF` to convert into Node
  2. Point newly converted Node's 64 children to the 64 newly formatted Buckets
  3. Rehash all 21 entries from converted Bucket into new child Buckets using hash bits at new depth
  4. Route and insert pending entry

### 5.3 DataBuf Expansion

- **Trigger**:
  Insufficient contiguous 16 B buffers in active DataBuf chain

- **Allocation**:
  Standard ($\le 252$ buffers): +1 OS page (4 DataBufs = 252 buffers)
  Jumbo ($> 252$ buffers): $+\lceil \text{buffers} / 252 \rceil$ OS pages

- New DataBufs are linked into Metadata's `databuf_head` chain

### 5.4 AdjArray Expansion

- **Trigger**:
  All 20-byte list arrays occupied across all AdjArray blocks

- **Allocation**:
  +1 OS page (4096 B = 32 blocks = 192 arrays)

- Linked into Metadata's `adj_arr_head` free chain

## 6. Concurrency & Transaction Model (OCC)

```
Reader:
  1. Read Slot via mmap.read(slot_idx) under per-slot read spinlock (torn-read immune)
  2. Non-blocking relative to other readers

Writer (Atomic Multi-Slot OCC):
  1. Read required Slots (Bucket, DataBuf bitmap, Payload, AdjArr)
  2. Compute new Slot bitstates and values
  3. tx = mmap.new_tx()
  4. Register expected vs new slot values: tx.write(&expected, &new_slot, slot_idx)
  5. Commit:
     a. Sort target slot indices ascending (Deadlock prevention)
     b. Acquire per-slot spinlocks in order
     c. CAS verification: for all slots, assert *slot_ptr == expected
     d. If CAS holds: memcpy new slots, advance write epoch, release locks -> Ok(AckTicket)
     e. If CAS fails: release locks, abort -> backoff (50 us..10 ms, +/-25% jitter), retry (max 32)
```

## 7. Operational Workflows

### 7.1 `get(key: &[u8]) -> Result<Option<Vec<u8>>>`

1. Compute `hash = hash_key(key)`
2. Descend Trie from `root_node_vp` while VP bit 31 == 0 (`(hash >> 6*D) & 0x3F`)
3. In Bucket (`bit 31 == 1`), probe 21 slots
4. On hash match:
   - If `buf_idx & 0x8000_0000 != 0`:
     Return `Err(InvalidType("Key is a list"))`
   - Read payload from DataBuf at `buf_idx & 0x7FFF_FFFF`
   - Validate CRC32 and key equality;
     return `Ok(Some(value_bytes))`
5. On `hash == 0` or 21 probes exhausted:
   Return `Ok(None)`

### 7.2 `set(key: &[u8], val: &[u8]) -> Result<TurboTicket>`

1. Traverse to target Bucket and slot
2. If update and `new_len <= old_allocated`:
   Overwrite in-place via OCC tx
3. If insert or payload expansion:
   - If Bucket full:
     Execute Bucket Split, re-locate
   - Allocate contiguous 16 B buffers in DataBuf
   - Build payload `[ CRC32 ][ Key ][ Val ][ Pad ]`
   - Atomic OCC commit:
     Update DataBuf bitmap, DataBuf buffer slots, Bucket entry

### 7.3 `del(key: &[u8]) -> Result<bool>`

1. Locate Bucket entry;
   if missing, return `Ok(false)`
2. Atomic OCC commit:
   - Set Bucket `entry.hash = 1` (tombstone)
   - Clear occupied bits in DataBuf bitmap
   - Decrement `Metadata.total_entries`
3. Return `Ok(true)`

### 7.4 List Operations (`TurboList`)

- Key stored in Bucket with `buf_idx |= 0x8000_0000`, `vlen = head_adjarr_idx`
- DataBuf stores only `[ CRC32 ][ Key ]`
- Value items stored as linked `AdjArrayList` nodes (3 items per array)
- `append`:
  Traverse or cache tail array;
  if full, allocate new array from AdjArray block and link `prev.next_arr = new_arr`
- `pop`:
  Read tail item, clear DataBuf buffer, clear entry;
  if array becomes empty, unlink and return array to block bitmap
- `first` / `last` / `get_at(idx)` / `iter`:
  Zero-copy indexed traversal over `(buf_idx, vlen)` pointers, validating CRC32 per item

## 8. Lifecycle & Versioning

### 8.1 Database Instantiation

Initialization delegates lifecycle and presence validation entirely to `MemMap`,

- `create(cfg)`:
  Initializes new database backing file through `MemMap::new(memmap_cfg)`;
  fails if file already exists on disk (`err::EXS`);
  formats initial 8-page topography (Metadata, Root Node, Buckets, DataBufs, AdjArrays);
  synchronizes initial state to disk writeback

- `open(cfg)`:
  Attaches to existing database file through `MemMap::open(memmap_cfg)`;
  fails if backing file does not exist (`err::NEX`) or path is directory (`err::PTH`);
  verifies minimum file size, physical page quantization, and slot alignment;
  validates Slot 0 `Metadata` magic signature and layout version

### 8.2 Versioned Mapping

`TurboCfg` accepts an explicit `version: u32` parameter forwarded to `MemMapCfg`,

- Filesystem naming follows `<stem>_<version>.<ext>` convention managed by `MemMap`
- Callers may maintain multiple isolated database instances across distinct schema versions;
  or allocate parallel sidecar databases against identical base directory paths
- Engine logic executes zero manual filesystem presence checks;
  file existence, creation exclusivity, and descriptor locking remain encapsulated in `MemMap`

## 9. Architectural Upper Bounds & Capacity Proof

### 9.1 Addressable Ceilings

The primary architectural ceiling stems from `buf_idx` encoding:
- Bit `0x1F` (bit 31) reserves discriminator flags (`LIST_FLAG` / occupancy)
- Bits `0..30` (31 bits) encode `global_buf_idx = dbuf_slot * 0x3F + local_idx`
- Maximum buffer index: $2^{31} - 1 = 0\text{x}7\text{FFF\_FFFF}$ ($2{,}147{,}483{,}647$)
- Maximum addressable DataBuf slot:
  $$\text{max\_slot} = \lfloor (2^{31} - 1) / 0\text{x}3\text{F} \rfloor = 34{,}087{,}042$$
- Maximum addressable DataBufs (8 slots / 1024 B per DataBuf):
  $$\text{max\_databufs} = \lfloor 34{,}087{,}042 / 8 \rfloor = 4{,}260{,}880$$
- Maximum backing database file size (quantized to 4096 B OS pages):
  $$\text{pages} = \lceil 34{,}087{,}042 / 32 \rceil = 1{,}065{,}221$$
  $$\text{bytes} = 1{,}065{,}221 \times 4096 = 4{,}363{,}145{,}216 \text{ B}$$
  ($\approx 4.0635 \text{ GiB}$)

### 9.2 Capacity Bounds for Common Payload Formats

- **KV Entries (12 B key + 48 B value $\implies$ 64 B frame $\implies$ 4 buffers)**:
  - Each DataBuf contains 63 buffers: $\lfloor 63 / 4 \rfloor = 15$ entries per DataBuf (3 slack)
  - Maximum total KV entries:
    $$4{,}260{,}880 \times 15 = 63{,}913{,}200 \quad (0\text{x}03\text{CF\_3CF0})$$

- **List Items (44 B value $\implies$ 48 B frame $\implies$ 3 buffers)**:
  - Each DataBuf fits $63 / 3 = 21$ item payloads ($0$ slack buffers)
  - Maximum aggregate list items:
    $$4{,}260{,}880 \times 21 = 89{,}478{,}480 \quad (0\text{x}0555\_5550)$$
  - Maximum items in a single list chain (bounded by `AdjArrayList.next_arr: u16`):
    $$(2^{16} - 1) \times 3 = 65{,}535 \times 3 = 196{,}605 \quad (0\text{x}0002\_\text{FFFD})$$

### 9.3 Verification Proof

```python
MAX_BUF_IDX = (1 << 31) - 1
BUFS_PER_DBUF = 0x3F
SLOTS_PER_DBUF = 8

max_dbuf_slot = MAX_BUF_IDX // BUFS_PER_DBUF
max_databufs = max_dbuf_slot // SLOTS_PER_DBUF

pages = (max_dbuf_slot + 31) // 32
file_bytes = pages * 4096

kv_limit = max_databufs * (BUFS_PER_DBUF // 4)
list_item_limit = max_databufs * (BUFS_PER_DBUF // 3)
single_list_limit = 0xFFFF * 3

assert max_dbuf_slot == 34087042
assert max_databufs == 4260880
assert file_bytes == 4363145216
assert kv_limit == 63913200
assert list_item_limit == 89478480
assert single_list_limit == 196605
```
