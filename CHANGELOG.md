# Changelog

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
