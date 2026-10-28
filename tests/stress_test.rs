use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

/// Ref: `DESIGN.md` §6
#[test]
fn ok_stress_concurrent_occ_contention() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let shared_count = 0x08;
    for i in 0..shared_count {
        let key = format!("shared_{:02x}", i);
        db.set(key.as_bytes(), b"initial_state").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut writer_handles = Vec::new();
    let mut reader_handles = Vec::new();

    // 0x04 writer threads mutating shared keys to stress OCC conflict retries
    for t in 0..0x04 {
        let db_clone = db.clone();
        writer_handles.push(thread::spawn(move || {
            for round in 0..0x40 {
                let idx = (t + round) % shared_count;
                let key = format!("shared_{:02x}", idx);
                let val = format!("worker_{:02x}_round_{:02x}", t, round);
                db_clone.set(key.as_bytes(), val.as_bytes()).unwrap();
            }
        }));
    }

    // 0x02 reader threads concurrently checking entries
    for _ in 0..0x02 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        reader_handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = iter % shared_count;
                let key = format!("shared_{:02x}", idx);
                if let Ok(Some(val)) = db_clone.get(key.as_bytes()) {
                    assert!(!val.is_empty());
                }
                iter = iter.wrapping_add(1);
            }
        }));
    }

    for h in writer_handles {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in reader_handles {
        h.join().unwrap();
    }

    for i in 0..shared_count {
        let key = format!("shared_{:02x}", i);
        assert!(db.get(key.as_bytes()).unwrap().is_some());
    }

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §6, §7.2, §7.4
#[test]
fn ok_stress_mpmc_shared_queue_pipeline() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let queue_name = b"shared_mpmc_queue";
    let running = Arc::new(AtomicBool::new(true));
    let popped_count = Arc::new(AtomicUsize::new(0));
    let mut pusher_handles = Vec::new();
    let mut popper_handles = Vec::new();

    // 0x02 pusher threads appending to a single shared queue
    for t in 0..0x02 {
        let db_clone = db.clone();
        pusher_handles.push(thread::spawn(move || {
            let queue = db_clone.list(queue_name);
            for i in 0..0x80 {
                let item = format!("p_{:02x}_item_{:04x}", t, i);
                queue.append(item.as_bytes()).unwrap();
            }
        }));
    }

    // 0x02 popper threads draining from the same shared queue
    for _ in 0..0x02 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        let count_clone = popped_count.clone();
        popper_handles.push(thread::spawn(move || {
            let queue = db_clone.list(queue_name);
            while running_clone.load(Ordering::Relaxed) {
                if let Ok(Some(_)) = queue.pop() {
                    count_clone.fetch_add(1, Ordering::Relaxed);
                } else {
                    thread::yield_now();
                }
            }
        }));
    }

    for h in pusher_handles {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in popper_handles {
        h.join().unwrap();
    }

    // Drain remaining items from queue within scoped block so handle drops
    {
        let queue = db.list(queue_name);
        while let Ok(Some(_)) = queue.pop() {
            popped_count.fetch_add(1, Ordering::Relaxed);
        }

        assert_eq!(popped_count.load(Ordering::Relaxed), 0x0100);
        assert_eq!(queue.len().unwrap(), 0);
    }

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.3, §4.2, §6
#[test]
fn ok_stress_concurrent_read_write_delete_churn() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let key_space = 0x40;
    for i in 0..key_space {
        let k = format!("churn_key_{:02x}", i);
        db.set(k.as_bytes(), b"initial_seed_value").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut mutator_handles = Vec::new();
    let mut reader_handles = Vec::new();

    // 0x02 writer threads mutating keys
    for t in 0..0x02 {
        let db_clone = db.clone();
        mutator_handles.push(thread::spawn(move || {
            for i in 0..0x60 {
                let idx = (t * 0x20 + i) % key_space;
                let k = format!("churn_key_{:02x}", idx);
                let v = format!("writer_{:02x}_mutation_{:04x}", t, i);
                let _ = db_clone.set(k.as_bytes(), v.as_bytes());
            }
        }));
    }

    // 0x02 deleter threads deleting keys to force slot recycling and tombstoning
    for t in 0..0x02 {
        let db_clone = db.clone();
        mutator_handles.push(thread::spawn(move || {
            for i in 0..0x60 {
                let idx = (t * 0x10 + i) % key_space;
                let k = format!("churn_key_{:02x}", idx);
                let _ = db_clone.del(k.as_bytes());
            }
        }));
    }

    // 0x02 reader threads checking entries concurrently
    for _ in 0..0x02 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        reader_handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = iter % key_space;
                let k = format!("churn_key_{:02x}", idx);
                let _ = db_clone.get(k.as_bytes());
                iter = iter.wrapping_add(1);
            }
        }));
    }

    for h in mutator_handles {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in reader_handles {
        h.join().unwrap();
    }

    // DB remains operable after churn
    let test_key = b"churn_post_verification";
    db.set(test_key, b"ok").unwrap();
    assert_eq!(db.get(test_key).unwrap(), Some(b"ok".to_vec()));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §5.2, §5.4, §6
#[test]
fn ok_stress_multi_quantum_backing_file_and_trie_growth() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let initial_pages = db.total_pages().unwrap();
    let seed_count = 0x10;
    for i in 0..seed_count {
        let k = format!("seed_{:04x}", i);
        db.set(k.as_bytes(), b"baseline").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut writer_handles = Vec::new();
    let mut reader_handles = Vec::new();

    // 0x02 readers concurrently querying seed and growth keys
    for _ in 0..0x02 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        reader_handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = iter % seed_count;
                let k = format!("seed_{:04x}", idx);
                let res = db_clone.get(k.as_bytes());
                assert!(res.is_ok());
                iter = iter.wrapping_add(1);
            }
        }));
    }

    // 0x02 writer threads creating heavy payload entries to force multi-quantum expansions
    for t in 0..0x02 {
        let db_clone = db.clone();
        writer_handles.push(thread::spawn(move || {
            for i in 0..0x80 {
                let k = format!("growth_w{:02x}_{:04x}", t, i);
                let v = vec![((t ^ i) & 0xFF) as u8; 0x40];
                db_clone.set(k.as_bytes(), &v).unwrap();
            }
        }));
    }

    for h in writer_handles {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in reader_handles {
        h.join().unwrap();
    }

    let grown_pages = db.total_pages().unwrap();
    assert!(grown_pages > initial_pages);
    assert_eq!(db.total_entries(), (seed_count + 0x0100) as u64);

    // Validate payloads for written entries
    for t in 0..0x02 {
        for i in 0..0x80 {
            let k = format!("growth_w{:02x}_{:04x}", t, i);
            let expected = vec![((t ^ i) & 0xFF) as u8; 0x40];
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected));
        }
    }

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §6, §7.4, §8.1
#[test]
fn ok_stress_reopen_and_crc_integrity() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg.clone()).unwrap();

    let mut handles = Vec::new();

    // 0x02 KV workers
    for t in 0..0x02 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for i in 0..0x40 {
                let k = format!("reopen_kv_{:02x}_{:04x}", t, i);
                let v = format!("payload_crc_val_{:02x}_{:04x}", t, i);
                db_clone.set(k.as_bytes(), v.as_bytes()).unwrap();
            }
        }));
    }

    // 0x02 list workers
    for t in 0..0x02 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            let queue = db_clone.list(format!("reopen_list_{:02x}", t).as_bytes());
            for i in 0..0x20 {
                let item = format!("reopen_item_{:02x}_{:04x}", t, i);
                queue.append(item.as_bytes()).unwrap();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // 2 * 0x40 KV entries + 2 lists = 0x80 + 2 = 0x82 entries
    assert_eq!(db.total_entries(), 0x82);
    db.sync().unwrap();
    drop(db);

    // Reopen and verify data and CRC integrity
    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 0x82);

    for t in 0..0x02 {
        for i in 0..0x40 {
            let k = format!("reopen_kv_{:02x}_{:04x}", t, i);
            let expected = format!("payload_crc_val_{:02x}_{:04x}", t, i);
            assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(expected.into_bytes()));
        }

        let queue = reopened.list(format!("reopen_list_{:02x}", t).as_bytes());
        assert_eq!(queue.len().unwrap(), 0x20);
        for i in 0..0x20 {
            let expected = format!("reopen_item_{:02x}_{:04x}", t, i);
            assert_eq!(queue.get_at(i).unwrap(), Some(expected.into_bytes()));
        }
    }

    reopened.delete().unwrap();
}
