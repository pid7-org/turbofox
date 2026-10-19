use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox, TurboTicket};

#[test]
fn ok_turbofox_basic_set_get() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));
    let db = TurboFox::create(cfg).unwrap();

    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(b"alpha").unwrap(), None);

    let ticket: TurboTicket = db.set(b"alpha", b"val_one").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"alpha").unwrap(), Some(b"val_one".to_vec()));

    let ticket2 = db.set(b"beta", b"val_two").unwrap();
    assert!(ticket2.epoch() >= ticket.epoch());
    assert_eq!(db.total_entries(), 2);
    assert_eq!(db.get(b"beta").unwrap(), Some(b"val_two".to_vec()));
    assert_eq!(db.get(b"alpha").unwrap(), Some(b"val_one".to_vec()));

    db.sync().unwrap();
    assert!(ticket.is_durable());
    assert_eq!(ticket.wait().unwrap(), ticket.epoch());
    assert!(ticket2.is_durable());
    assert_eq!(ticket2.wait().unwrap(), ticket2.epoch());

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_inplace_and_expansion_overwrite() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));
    let db = TurboFox::create(cfg).unwrap();

    db.set(b"key_one", b"initial_payload").unwrap();
    assert_eq!(db.get(b"key_one").unwrap(), Some(b"initial_payload".to_vec()));
    assert_eq!(db.total_entries(), 1);

    // In-place overwrite with smaller payload
    db.set(b"key_one", b"tiny").unwrap();
    assert_eq!(db.get(b"key_one").unwrap(), Some(b"tiny".to_vec()));
    assert_eq!(db.total_entries(), 1);

    // Expansion overwrite with much larger payload
    let large_val = vec![0x2A; 0x0180];
    db.set(b"key_one", &large_val).unwrap();
    assert_eq!(db.get(b"key_one").unwrap(), Some(large_val));
    assert_eq!(db.total_entries(), 1);

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_databuf_dynamic_growth() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));
    let db = TurboFox::create(cfg).unwrap();

    // Initial 14 DataBufs provide 882 16-byte buffers
    // Inserting 0x64 items of ~0x60 bytes each will saturate the pool and trigger expansion
    for i in 0..0x64 {
        let key = format!("bulk_key_{:04x}", i);
        let val = vec![(i & 0xFF) as u8; 0x70];
        db.set(key.as_bytes(), &val).unwrap();
    }

    assert_eq!(db.total_entries(), 0x64);

    // Verify all keys remain intact after growth
    for i in 0..0x64 {
        let key = format!("bulk_key_{:04x}", i);
        let expected = vec![(i & 0xFF) as u8; 0x70];
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
    }

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_bucket_split_trie_deepening() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));
    let db = TurboFox::create(cfg).unwrap();

    // 64 buckets with capacity 0x15 each; 64 * 0x15 = 1344 (0x0540)
    // 0x0550 keys guarantees bucket split by pigeonhole principle
    let count: usize = 0x0550;

    for i in 0..count {
        let key = format!("split_key_{:04x}", i);
        let val = format!("val_{:04x}", i);
        db.set(key.as_bytes(), val.as_bytes()).unwrap();
    }

    assert_eq!(db.total_entries(), count as u64);

    for i in 0..count {
        let key = format!("split_key_{:04x}", i);
        let val = format!("val_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(val.into_bytes()));
    }

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_reopen_and_persistence() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.set(b"durable_key", b"durable_val").unwrap();
        db.sync().unwrap();
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 1);
    assert_eq!(reopened.get(b"durable_key").unwrap(), Some(b"durable_val".to_vec()));

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_concurrent_writers() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("test.db"));
    let db = TurboFox::create(cfg).unwrap();

    let mut handles = Vec::new();
    let num_threads = 4;
    let items_per_thread = 0x20;

    for t in 0..num_threads {
        let db_clone = db.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..items_per_thread {
                let key = format!("thread_{}_key_{}", t, i);
                let val = format!("thread_{}_val_{}", t, i);
                db_clone.set(key.as_bytes(), val.as_bytes()).unwrap();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(db.total_entries(), (num_threads * items_per_thread) as u64);

    for t in 0..num_threads {
        for i in 0..items_per_thread {
            let key = format!("thread_{}_key_{}", t, i);
            let val = format!("thread_{}_val_{}", t, i);
            assert_eq!(db.get(key.as_bytes()).unwrap(), Some(val.into_bytes()));
        }
    }

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_full_update_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("lifecycle.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();

        // 1: Initial Set
        db.set(b"lifecycle_key", b"v1_initial").unwrap();
        assert_eq!(db.total_entries(), 1);
        assert_eq!(db.get(b"lifecycle_key").unwrap(), Some(b"v1_initial".to_vec()));

        // 2: In-place Update with matched length
        db.set(b"lifecycle_key", b"v2_matched").unwrap();
        assert_eq!(db.total_entries(), 1);
        assert_eq!(db.get(b"lifecycle_key").unwrap(), Some(b"v2_matched".to_vec()));

        // 3: Shrinking Update
        db.set(b"lifecycle_key", b"v3").unwrap();
        assert_eq!(db.total_entries(), 1);
        assert_eq!(db.get(b"lifecycle_key").unwrap(), Some(b"v3".to_vec()));

        // 4: Expanding Update
        let expanded = vec![0x3C; 0x0120];
        db.set(b"lifecycle_key", &expanded).unwrap();
        assert_eq!(db.total_entries(), 1);
        assert_eq!(db.get(b"lifecycle_key").unwrap(), Some(expanded.clone()));

        db.sync().unwrap();
    }

    // Verify durability and persistence of updated key across reopen
    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 1);
    let expected = vec![0x3C; 0x0120];
    assert_eq!(reopened.get(b"lifecycle_key").unwrap(), Some(expected));

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_multibuf_large_spanned_payloads() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("spanned.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();

        // 0x0800 bytes spanning across multiple DataBufs
        let payload_2k = vec![0x41; 0x0800];
        db.set(b"spanned_2k", &payload_2k).unwrap();
        assert_eq!(db.get(b"spanned_2k").unwrap(), Some(payload_2k.clone()));

        // 0x4000 bytes triggering multi-page dynamic growth
        let payload_16k = vec![0x42; 0x4000];
        db.set(b"spanned_16k", &payload_16k).unwrap();
        assert_eq!(db.get(b"spanned_16k").unwrap(), Some(payload_16k.clone()));

        // Overwrite expansion to 0x8000 bytes
        let payload_32k = vec![0x43; 0x8000];
        db.set(b"spanned_2k", &payload_32k).unwrap();
        assert_eq!(db.get(b"spanned_2k").unwrap(), Some(payload_32k));

        // Overwrite shrink down to 0x20 bytes
        let small_val = vec![0x44; 0x20];
        db.set(b"spanned_16k", &small_val).unwrap();
        assert_eq!(db.get(b"spanned_16k").unwrap(), Some(small_val));

        db.sync().unwrap();
    }

    // Reopen and verify persistence of multi-databuf entries
    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 2);
    let expected_32k = vec![0x43; 0x8000];
    assert_eq!(reopened.get(b"spanned_2k").unwrap(), Some(expected_32k));
    let expected_small = vec![0x44; 0x20];
    assert_eq!(reopened.get(b"spanned_16k").unwrap(), Some(expected_small));

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_del_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("del_lifecycle.db"));
    let db = TurboFox::create(cfg).unwrap();

    assert_eq!(db.del(b"missing").unwrap(), false);
    assert_eq!(db.total_entries(), 0);

    db.set(b"k1", b"val1").unwrap();
    db.set(b"k2", b"val2").unwrap();
    db.set(b"k3", b"val3").unwrap();
    assert_eq!(db.total_entries(), 3);

    assert_eq!(db.del(b"k2").unwrap(), true);
    assert_eq!(db.total_entries(), 2);
    assert_eq!(db.get(b"k2").unwrap(), None);
    assert_eq!(db.get(b"k1").unwrap(), Some(b"val1".to_vec()));
    assert_eq!(db.get(b"k3").unwrap(), Some(b"val3".to_vec()));

    // Repeated del on already deleted key
    assert_eq!(db.del(b"k2").unwrap(), false);
    assert_eq!(db.total_entries(), 2);

    assert_eq!(db.del(b"k1").unwrap(), true);
    assert_eq!(db.del(b"k3").unwrap(), true);
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(b"k1").unwrap(), None);
    assert_eq!(db.get(b"k3").unwrap(), None);

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_del_tombstone_reuse_and_reinsert() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("tombstone_reuse.db"));
    let db = TurboFox::create(cfg).unwrap();

    db.set(b"reuse_key", b"initial").unwrap();
    assert_eq!(db.get(b"reuse_key").unwrap(), Some(b"initial".to_vec()));
    assert_eq!(db.total_entries(), 1);

    assert_eq!(db.del(b"reuse_key").unwrap(), true);
    assert_eq!(db.get(b"reuse_key").unwrap(), None);
    assert_eq!(db.total_entries(), 0);

    // Re-inserting previously deleted key reuses tombstone slot
    db.set(b"reuse_key", b"reborn").unwrap();
    assert_eq!(db.get(b"reuse_key").unwrap(), Some(b"reborn".to_vec()));
    assert_eq!(db.total_entries(), 1);

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_del_persistence() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("del_persist.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.set(b"survivor", b"persisted_val").unwrap();
        db.set(b"doomed", b"temporary_val").unwrap();
        assert_eq!(db.total_entries(), 2);

        assert_eq!(db.del(b"doomed").unwrap(), true);
        assert_eq!(db.total_entries(), 1);
        db.sync().unwrap();
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 1);
    assert_eq!(reopened.get(b"survivor").unwrap(), Some(b"persisted_val".to_vec()));
    assert_eq!(reopened.get(b"doomed").unwrap(), None);

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_del_reclaims_databuf_without_growth() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("reclaim_nogrowth.db"));
    let db = TurboFox::create(cfg).unwrap();

    // Fill initial pool with 0x20 keys having 0x30 bytes payload
    for i in 0..0x20 {
        let key = format!("pre_reclaim_key_{:04x}", i);
        let val = vec![0xAA; 0x30];
        db.set(key.as_bytes(), &val).unwrap();
    }
    assert_eq!(db.total_entries(), 0x20);

    // Delete half of them (0x10 keys)
    for i in 0..0x10 {
        let key = format!("pre_reclaim_key_{:04x}", i);
        assert_eq!(db.del(key.as_bytes()).unwrap(), true);
    }
    assert_eq!(db.total_entries(), 0x10);

    // Insert 0x10 new keys into the reclaimed space
    for i in 0..0x10 {
        let key = format!("post_reclaim_key_{:04x}", i);
        let val = vec![0xBB; 0x30];
        db.set(key.as_bytes(), &val).unwrap();
    }
    assert_eq!(db.total_entries(), 0x20);

    // Verify both surviving and new keys are readable
    for i in 0x10..0x20 {
        let key = format!("pre_reclaim_key_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(vec![0xAA; 0x30]));
    }
    for i in 0..0x10 {
        let key = format!("post_reclaim_key_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(vec![0xBB; 0x30]));
    }

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_del_probe_across_consecutive_tombstones() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("tombstone_chain.db"));
    let db = TurboFox::create(cfg).unwrap();

    // Insert 0x0A keys
    for i in 0..0x0A {
        let key = format!("probe_chain_k_{:02x}", i);
        let val = format!("val_{:02x}", i);
        db.set(key.as_bytes(), val.as_bytes()).unwrap();
    }

    // Delete first 0x05 keys creating consecutive tombstones in probe paths
    for i in 0..0x05 {
        let key = format!("probe_chain_k_{:02x}", i);
        assert_eq!(db.del(key.as_bytes()).unwrap(), true);
    }

    // Verify trailing keys are still reachable across tombstones
    for i in 0x05..0x0A {
        let key = format!("probe_chain_k_{:02x}", i);
        let val = format!("val_{:02x}", i);
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(val.into_bytes()));
    }

    // Delete remaining keys across tombstones
    for i in 0x05..0x0A {
        let key = format!("probe_chain_k_{:02x}", i);
        assert_eq!(db.del(key.as_bytes()).unwrap(), true);
    }
    assert_eq!(db.total_entries(), 0);

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_max_payload_roundtrip_and_del() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("max_payload.db"));
    let db = TurboFox::create(cfg).unwrap();

    // Max allowed key (0x0100 B) and max allowed payload (0x8000 B)
    let key = vec![0x4B; 0x0100];
    let val = vec![0x56; 0x8000];

    db.set(&key, &val).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(&key).unwrap(), Some(val.clone()));

    assert_eq!(db.del(&key).unwrap(), true);
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(&key).unwrap(), None);

    db.delete().unwrap();
}

#[test]
fn err_turbofox_oversized_key_del_and_get() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("oversized.db"));
    let db = TurboFox::create(cfg).unwrap();

    // Key exceeding u16::MAX (0x0001_0000 bytes)
    let huge_key = vec![0x58; 0x0001_0000];

    assert_eq!(db.get(&huge_key).unwrap(), None);
    assert_eq!(db.del(&huge_key).unwrap(), false);

    db.delete().unwrap();
}

#[test]
fn ok_turbofox_capacity_metrics_and_invariants() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("cap.db"));
    let db = TurboFox::create(cfg).unwrap();

    let init_pages = db.total_pages().unwrap();
    assert_eq!(init_pages, 8);

    let remaining_pages = db.remaining_pages().unwrap();
    assert_eq!(remaining_pages, 0x0010_4104 - 8);

    let remaining_bytes = db.remaining_bytes().unwrap();
    assert_eq!(remaining_bytes, (remaining_pages as u64) * 0x1000);

    db.delete().unwrap();
}
