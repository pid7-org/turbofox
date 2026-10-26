# Changelog

## [0.0.101] - 2026-10-26

- Introduction of the new v2 storage layer
- **Breaking:** Previous v1 storage layer will not be supported
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
