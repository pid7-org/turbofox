# Changelog

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
