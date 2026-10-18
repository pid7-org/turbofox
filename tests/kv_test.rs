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
