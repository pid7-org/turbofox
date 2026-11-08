# Changelog

## [0.0.301] - 2026-11-08

- **Breaking:** Minimum Supported Rust Version (MSRV) raised to 1.89.0
- **Breaking:** Databases created with previous versions are incompatible and unsupported
- AVX-512F vector probing kernels and dynamic ISA dispatch for x86_64 architectures
- 32-entry (`0x20`) bucket SIMD probing to accelerate hash matching and reduce trie depth
- Bit 31 keyspace partitioning isolating key-value and sequence domains against cross collisions
- Demand-paged `TurboListIter` streaming item payloads lazily with O(1) memory overhead
- Fail-safe iterator traversal terminating gracefully upon concurrent sequence deletion or pop
- `TurboList`:
  - `del` method for removing an entire sequence and freeing descriptor and payload buffers

## [0.0.300] - 2026-11-04

- **Breaking:** Databases created with previous versions are incompatible and unsupported
- Dynamic 15-way trie routing with on-demand node expansion, replacing flat bucket tables
- 64-byte slot topology with quantized 0x0100 allocation blocks to eliminate internal fragmentation
- Multi-DBuf payload allocation supporting KV values and list items spanning up to 0xFFFF bytes
- Upfront capacity hinting with physical quantum alignment to bypass runtime resize latency
- Optional in-memory lookup cache (`lookup_cache`) accelerating DBuf slot resolution
- Advisory kernel page eviction (`evict`) with header-preserving and full-purge operational modes
- Zero-cost sync barrier execution when database pages remain clean or evicted
- `TurboFox`:
  - `open_or_create` constructor for opening existing or initializing new databases
  - `evict` for advisory page cache purging via `EvictionMode`
  - `resident_bytes_in_memory` telemetry for probing mapped physical RAM footprint
  - `resident_bytes_on_disk` telemetry for active backing file allocation length
  - Removed `total_pages`, `remaining_pages`, and `remaining_bytes`
- `TurboCfg`:
  - `capacity_hint` field and `with_capacity_hint` builder method
  - `lookup_cache` field and `with_lookup_cache` builder method
  - Removed `mem_index` and `with_mem_index`
  - Removed `with_sync_interval`
- `CapacityHint`:
  - Pre-allocation configuration builder (`new`, `with_kv`, `with_list`)
- `EvictionMode`:
  - Cache shedding policies (`PreserveHeaders`, `PurgeAll`)
- `TurboError`:
  - `CollisionLimit` variant for maximum trie recursion depth ceiling

## [0.0.201] - 2026-10-27

- **Breaking:** Databases created with previous versions are incompatible and unsupported
- Optional in-memory SIMD fingerprint index for key existence and databuf slot resolution
- 16-slot open addressing with runtime ISA dispatch (AVX2, SSE2, NEON, scalar)
- Adaptive spin-yield OCC backoff with jitter and expanded 0x100 retry budget under contention
- `TurboCfg`:
  - `mem_index` flag for configuring in-memory index acceleration
  - `with_mem_index` builder method

## [0.0.101] - 2026-10-26

- **Breaking:** Databases created with previous versions are incompatible and unsupported
- `MAX_KEY_LEN` and `MAX_VAL_LEN` payload limits
- `TurboFox`
  - `is_empty` empty state check
  - `contains_key` key existence probe
- `TurboCfg`
  - `path` expects a directory path instead of a file path; `version` parameter removed

## [0.0.11] - 2026-10-21

- Initial release
- `TurboFox`:
  - `create` and `open` single-file persistent database instances
  - atomic key-value operations: `get`, `set`, and `del`
  - `sync` disk flush with durability state verification via `TurboTicket`
- `TurboList`:
  - persistent sequence operations: `append`, `pop`, and `len`
  - ordered forward iteration over stored items
- `TurboCfg`:
  - storage path and engine initialization parameters
