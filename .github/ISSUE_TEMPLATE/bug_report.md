---
name: Bug report
about: Report unexpected engine errors, corruptions, or invariant violations
title: 'bug: '
labels: bug
assignees: adityamotale
type: Bug
---

## Summary

Provide a concise description of the bug or unexpected failure.

## Error Details

If applicable, provide the full `TurboError` variant and context:

```rust
TurboError::CorruptedPayload("KV CRC32 mismatch")
```

## Engine Configuration

Provide relevant configuration parameters:

- Storage mode: `TurboCfg::manual` / `TurboCfg::periodic`
- Lookup cache: enabled / disabled
- Capacity hint: entries, key size, value size
- Target filesystem: (e.g. XFS, ext4, APFS, NTFS)

## Host Environment

- Operating System: (e.g. Linux 6.18, macOS 15.4, Windows 11)
- Architecture: (`x86_64` / `aarch64`)
- Host Page Size: (4 KiB, 16 KiB, 64 KiB)
- CPU Model & ISA: (e.g. Intel Xeon Platinum, AVX2 / AVX-512 / NEON)
- Rust Version: (`rustc --version`)
- TurboFox Version: (e.g. `0.0.301`)

## Reproduction

Minimal reproducible snippet:

```rust
use turbofox::{TurboCfg, TurboFox};

let dir = tempfile::tempdir().unwrap();
let db = TurboFox::open_or_create(TurboCfg::manual(dir.path())).unwrap();
// Add reproduction steps here
```

## Expected Behavior

Describe what should have occurred.

## Actual Behavior

Describe observed behavior, panics, or corrupted entries.

## Logs / Diagnostics

Attach crash logs, strace output, or backtraces (`RUST_BACKTRACE=1`).
