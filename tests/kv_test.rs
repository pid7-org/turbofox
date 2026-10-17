//! Integration tests for TurboFox KV set and get operations

use turbofox::{TurboCfg, TurboFox};

struct TempDb {
    path: std::path::PathBuf,
    versioned: std::path::PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        path.push(format!("tf_kv_{}_{}_{}.db", name, pid, ts));
        let versioned = TurboCfg::compute_versioned_path(&path, 1);
        if versioned.exists() {
            let _ = std::fs::remove_file(&versioned);
        }
        Self { path, versioned }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        if self.versioned.exists() {
            let _ = std::fs::remove_file(&self.versioned);
        }
    }
}

#[test]
fn ok_turbofox_basic_set_get() {
    let temp = TempDb::new("basic_set_get");
    let cfg = TurboCfg::new(&temp.path);
    let db = TurboFox::new(cfg).unwrap();

    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(b"alpha").unwrap(), None);

    let ticket = db.set(b"alpha", b"val_one").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"alpha").unwrap(), Some(b"val_one".to_vec()));

    let ticket2 = db.set(b"beta", b"val_two").unwrap();
    assert!(ticket2.epoch() >= ticket.epoch());
    assert_eq!(db.total_entries(), 2);
    assert_eq!(db.get(b"beta").unwrap(), Some(b"val_two".to_vec()));
    assert_eq!(db.get(b"alpha").unwrap(), Some(b"val_one".to_vec()));
}

#[test]
fn ok_turbofox_inplace_and_expansion_overwrite() {
    let temp = TempDb::new("overwrite");
    let cfg = TurboCfg::new(&temp.path);
    let db = TurboFox::new(cfg).unwrap();

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
}

#[test]
fn ok_turbofox_databuf_dynamic_growth() {
    let temp = TempDb::new("databuf_growth");
    let cfg = TurboCfg::new(&temp.path);
    let db = TurboFox::new(cfg).unwrap();

    let initial_slots = db.total_slots();
    assert_eq!(initial_slots, 0x0100);

    // Initial 14 DataBufs provide 882 16-byte buffers
    // Inserting 0x64 items of ~0x60 bytes each will saturate the pool and trigger expansion
    for i in 0..0x64 {
        let key = format!("bulk_key_{:04x}", i);
        let val = vec![(i & 0xFF) as u8; 0x70];
        db.set(key.as_bytes(), &val).unwrap();
    }

    assert_eq!(db.total_entries(), 0x64);
    assert!(db.total_slots() > initial_slots);

    // Verify all keys remain intact after growth
    for i in 0..0x64 {
        let key = format!("bulk_key_{:04x}", i);
        let expected = vec![(i & 0xFF) as u8; 0x70];
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
    }
}

#[test]
fn ok_turbofox_bucket_split_trie_deepening() {
    let temp = TempDb::new("bucket_split");
    let cfg = TurboCfg::new(&temp.path);
    let db = TurboFox::new(cfg).unwrap();

    let initial_slots = db.total_slots();

    // Generate keys that all route to the same initial root bucket (child_idx == 0)
    let mut colliding_keys = Vec::new();
    let mut counter: usize = 0;
    while colliding_keys.len() < 0x18 {
        let candidate = format!("col_key_{:06x}", counter);
        if (turbofox::hash::hash_key(candidate.as_bytes()) & 0x3F) == 0 {
            colliding_keys.push(candidate);
        }
        counter += 1;
    }

    // Insert 0x18 (24) keys into the same bucket (capacity 21) -> forces bucket split
    for (i, key) in colliding_keys.iter().enumerate() {
        let val = format!("val_{:04x}", i);
        db.set(key.as_bytes(), val.as_bytes()).unwrap();
    }

    assert_eq!(db.total_entries(), 0x18);
    // Bucket split adds 0x80 slots (4 physical OS pages)
    assert_eq!(db.total_slots(), initial_slots + 0x80);

    for (i, key) in colliding_keys.iter().enumerate() {
        let val = format!("val_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).unwrap(), Some(val.into_bytes()));
    }
}

#[test]
fn ok_turbofox_reopen_and_persistence() {
    let temp = TempDb::new("reopen_persist");
    let cfg = TurboCfg::new(&temp.path);

    {
        let db = TurboFox::new(cfg.clone()).unwrap();
        db.set(b"durable_key", b"durable_val").unwrap();
        db.sync().unwrap();
    }

    let reopened = TurboFox::new(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 1);
    assert_eq!(reopened.get(b"durable_key").unwrap(), Some(b"durable_val".to_vec()));
}

#[test]
fn ok_turbofox_concurrent_writers() {
    let temp = TempDb::new("concurrent_writers");
    let cfg = TurboCfg::new(&temp.path);
    let db = TurboFox::new(cfg).unwrap();

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
}
