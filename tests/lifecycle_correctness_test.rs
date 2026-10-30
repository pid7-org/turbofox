use tempfile::tempdir;
use turbofox::{EvictionMode, TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §3.1, §7.1, §7.2, §7.3
#[test]
fn ok_lifecycle_sequential_crud_full_cycle() {
    for &lookup_cache in &[false, true] {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);
        let db = TurboFox::create(cfg).unwrap();

        let n = 0x0800;
        let m = 0x0400;

        assert!(db.is_empty());
        assert_eq!(db.total_entries(), 0);

        // Phase 1: Sequential insertions of N keys
        for i in 0..n {
            let k = format!("k_{:04x}", i);
            let v = format!("val_initial_{:04x}", i);
            let ticket = db.set(k.as_bytes(), v.as_bytes()).unwrap();

            assert!(ticket.epoch() > 0);
        }

        assert_eq!(db.total_entries(), n as u64);
        assert!(!db.is_empty());

        // Phase 2: Sequential verification of all N keys
        for i in 0..n {
            let k = format!("k_{:04x}", i);
            let expected = format!("val_initial_{:04x}", i);

            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            assert!(db.contains_key(k.as_bytes()).unwrap());
        }

        // Phase 3: Mutate M existing keys with both smaller and larger payloads
        for i in 0..m {
            let k = format!("k_{:04x}", i);

            let updated_v = if i % 2 == 0 {
                // Smaller payload
                format!("s_{:02x}", i)
            } else {
                // Larger payload requiring slot re-allocation
                format!("large_mutated_payload_expansion_{:04x}_{}", i, "X".repeat(0x50))
            };

            db.set(k.as_bytes(), updated_v.as_bytes()).unwrap();
        }

        assert_eq!(db.total_entries(), n as u64);

        // Phase 4: Verify M mutated keys and N - M untouched keys
        for i in 0..m {
            let k = format!("k_{:04x}", i);

            let expected = if i % 2 == 0 {
                format!("s_{:02x}", i)
            } else {
                format!("large_mutated_payload_expansion_{:04x}_{}", i, "X".repeat(0x50))
            };

            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        for i in m..n {
            let k = format!("k_{:04x}", i);
            let expected = format!("val_initial_{:04x}", i);
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        // Phase 5: Sequential deletion of all N keys
        for i in 0..n {
            let k = format!("k_{:04x}", i);
            assert!(db.del(k.as_bytes()).unwrap());

            // Duplicate deletion must return false
            assert!(!db.del(k.as_bytes()).unwrap());
        }

        // Phase 6: Post-deletion verification
        assert_eq!(db.total_entries(), 0);
        assert!(db.is_empty());

        for i in 0..n {
            let k = format!("k_{:04x}", i);

            assert_eq!(db.get(k.as_bytes()).unwrap(), None);
            assert!(!db.contains_key(k.as_bytes()).unwrap());
        }

        db.delete().unwrap();
    }
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn ok_lifecycle_multicycle_reopen_persistence() {
    for &lookup_cache in &[false, true] {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);

        let n = 0x0600;
        let m = 0x0300;

        // Cycle 1: Create, insert N entries, sync, drop
        {
            let db = TurboFox::create(cfg.clone()).unwrap();
            for i in 0..n {
                let k = format!("persist_{:04x}", i);
                let v = format!("payload_cycle1_{:04x}", i);

                db.set(k.as_bytes(), v.as_bytes()).unwrap();
            }

            assert_eq!(db.total_entries(), n as u64);
            db.sync().unwrap();
        }

        // Cycle 2: Reopen, verify N entries, mutate M entries, sync, drop
        {
            let db = TurboFox::open(cfg.clone()).unwrap();
            assert_eq!(db.total_entries(), n as u64);

            for i in 0..n {
                let k = format!("persist_{:04x}", i);
                let expected = format!("payload_cycle1_{:04x}", i);

                assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            }

            for i in 0..m {
                let k = format!("persist_{:04x}", i);
                let v = format!("cycle2_update_{:04x}_{}", i, "Y".repeat(0x40));

                db.set(k.as_bytes(), v.as_bytes()).unwrap();
            }

            assert_eq!(db.total_entries(), n as u64);
            db.sync().unwrap();
        }

        // Cycle 3: Reopen, verify M mutated and N - M untouched, delete all N, sync, drop
        {
            let db = TurboFox::open(cfg.clone()).unwrap();
            assert_eq!(db.total_entries(), n as u64);

            for i in 0..m {
                let k = format!("persist_{:04x}", i);
                let expected = format!("cycle2_update_{:04x}_{}", i, "Y".repeat(0x40));

                assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            }

            for i in m..n {
                let k = format!("persist_{:04x}", i);
                let expected = format!("payload_cycle1_{:04x}", i);

                assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
            }

            for i in 0..n {
                let k = format!("persist_{:04x}", i);
                assert!(db.del(k.as_bytes()).unwrap());
            }

            assert_eq!(db.total_entries(), 0);
            db.sync().unwrap();
        }

        // Cycle 4: Reopen, verify empty state and clean slot reuse
        {
            let db = TurboFox::open(cfg.clone()).unwrap();
            assert_eq!(db.total_entries(), 0);
            assert!(db.is_empty());

            for i in 0..n {
                let k = format!("persist_{:04x}", i);
                assert_eq!(db.get(k.as_bytes()).unwrap(), None);
            }

            // Verify fresh insertion after complete delete lifecycle
            db.set(b"reborn_key", b"reborn_val").unwrap();
            assert_eq!(db.total_entries(), 1);
            assert_eq!(db.get(b"reborn_key").unwrap(), Some(b"reborn_val".to_vec()));

            db.delete().unwrap();
        }
    }
}

/// Ref: `DESIGN.md` §5.1, §5.4, §6
#[test]
fn ok_lifecycle_growth_boundary_crud() {
    for &lookup_cache in &[false, true] {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);
        let db = TurboFox::create(cfg.clone()).unwrap();

        let initial_pages = db.total_pages().unwrap();
        let count = 0x0400;

        // Step 1: Ingest payload entries forcing dynamic page expansion
        for i in 0..count {
            let k = format!("growth_k_{:04x}", i);
            let v = vec![((i ^ 0x5A) & 0xFF) as u8; 0xC0];

            db.set(k.as_bytes(), &v).unwrap();
        }

        let grown_pages = db.total_pages().unwrap();
        assert!(grown_pages > initial_pages);
        assert_eq!(db.total_entries(), count as u64);

        // Step 2: Overwrite existing keys with larger payloads to force databuf reallocations
        for i in 0..(count / 2) {
            let k = format!("growth_k_{:04x}", i);
            let v = vec![((i ^ 0xA5) & 0xFF) as u8; 0xD8];

            db.set(k.as_bytes(), &v).unwrap();
        }

        assert_eq!(db.total_entries(), count as u64);

        // Step 3: Validate all keys across grown boundaries
        for i in 0..(count / 2) {
            let k = format!("growth_k_{:04x}", i);
            let expected = vec![((i ^ 0xA5) & 0xFF) as u8; 0xD8];

            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected));
        }

        for i in (count / 2)..count {
            let k = format!("growth_k_{:04x}", i);
            let expected = vec![((i ^ 0x5A) & 0xFF) as u8; 0xC0];

            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected));
        }

        // Step 4: Delete all keys and verify slot reclamation
        for i in 0..count {
            let k = format!("growth_k_{:04x}", i);
            assert!(db.del(k.as_bytes()).unwrap());
        }

        assert_eq!(db.total_entries(), 0);
        assert!(db.is_empty());

        // Step 5: Insert replacement keys to ensure freelist/tombstone slots operate correctly
        let post_count = 0x0100;
        for i in 0..post_count {
            let k = format!("reclaimed_{:04x}", i);
            let v = format!("reclaimed_val_{:04x}", i);

            db.set(k.as_bytes(), v.as_bytes()).unwrap();
        }

        assert_eq!(db.total_entries(), post_count as u64);

        db.sync().unwrap();
        drop(db);

        // Step 6: Reopen and confirm post-growth reclaimed entries
        let reopened = TurboFox::open(cfg).unwrap();
        assert_eq!(reopened.total_entries(), post_count as u64);

        for i in 0..post_count {
            let k = format!("reclaimed_{:04x}", i);
            let expected = format!("reclaimed_val_{:04x}", i);

            assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        reopened.delete().unwrap();
    }
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn ok_lifecycle_open_open_semantics() {
    for &lookup_cache in &[false, true] {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);

        // 1: Initialize with open_or_create
        {
            let db = TurboFox::open_or_create(cfg.clone()).unwrap();
            db.set(b"open_k1", b"open_v1").unwrap();
            db.sync().unwrap();
        }

        // 2: Subsequent open_or_create opens existing state
        {
            let db = TurboFox::open_or_create(cfg.clone()).unwrap();
            assert_eq!(db.total_entries(), 1);
            assert_eq!(db.get(b"open_k1").unwrap(), Some(b"open_v1".to_vec()));

            // Mutate and sync
            db.set(b"open_k2", b"open_v2").unwrap();
            db.sync().unwrap();
        }

        // 3: Verify standard open sees updates
        {
            let db = TurboFox::open(cfg.clone()).unwrap();

            assert_eq!(db.total_entries(), 2);
            assert_eq!(db.get(b"open_k1").unwrap(), Some(b"open_v1".to_vec()));
            assert_eq!(db.get(b"open_k2").unwrap(), Some(b"open_v2".to_vec()));

            // Create on existing path must fail
            assert!(matches!(
                TurboFox::create(cfg.clone()).unwrap_err(),
                TurboError::Storage { .. }
            ));

            db.delete().unwrap();
        }
    }
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn ok_lifecycle_page_eviction_modes_correctness() {
    for &lookup_cache in &[false, true] {
        let dir = tempdir().unwrap();
        let cfg = TurboCfg::manual(dir.path()).with_lookup_cache(lookup_cache);
        let db = TurboFox::create(cfg.clone()).unwrap();

        let n = 0x0200;

        // Step 1: Ingest entries to populate memory pages and cache
        for i in 0..n {
            let k = format!("evict_key_{:04x}", i);
            let v = format!("evict_payload_{:04x}_{}", i, "Z".repeat(0x30));
            db.set(k.as_bytes(), v.as_bytes()).unwrap();
        }

        db.sync().unwrap();
        let pre_evict_resident = db.resident_bytes_in_memory().unwrap();
        assert!(pre_evict_resident > 0);

        // Step 2: Evict data pages while preserving headers
        db.evict(EvictionMode::PreserveHeaders).unwrap();

        // Verify all keys remain intact and readable through demand paging
        for i in 0..n {
            let k = format!("evict_key_{:04x}", i);
            let expected = format!("evict_payload_{:04x}_{}", i, "Z".repeat(0x30));
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        // Step 3: Mutate existing keys and insert new keys
        for i in 0..(n / 2) {
            let k = format!("evict_key_{:04x}", i);
            let updated = format!("updated_after_preserve_{:04x}", i);
            db.set(k.as_bytes(), updated.as_bytes()).unwrap();
        }

        let extra_key = b"evict_extra_key";
        db.set(extra_key, b"extra_value").unwrap();
        assert_eq!(db.get(extra_key).unwrap(), Some(b"extra_value".to_vec()));

        // Step 4: Evict all pages across the entire DB
        db.evict(EvictionMode::PurgeAll).unwrap();

        // Step 5: Verify mutated and original keys post PurgeAll
        for i in 0..(n / 2) {
            let k = format!("evict_key_{:04x}", i);
            let expected = format!("updated_after_preserve_{:04x}", i);
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        for i in (n / 2)..n {
            let k = format!("evict_key_{:04x}", i);
            let expected = format!("evict_payload_{:04x}_{}", i, "Z".repeat(0x30));
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        assert_eq!(db.get(extra_key).unwrap(), Some(b"extra_value".to_vec()));

        // Step 6: Verify deletion works seamlessly after eviction
        assert!(db.del(extra_key).unwrap());
        assert_eq!(db.get(extra_key).unwrap(), None);

        db.delete().unwrap();
    }
}
