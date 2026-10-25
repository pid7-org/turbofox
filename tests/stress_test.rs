use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    let mut handles = Vec::new();

    // 0x04 writer threads mutating shared keys to stress OCC conflict retries
    for t in 0..0x04 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for round in 0..0x18 {
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
        handles.push(thread::spawn(move || {
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

    for h in handles.drain(0..0x04) {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }

    for i in 0..shared_count {
        let key = format!("shared_{:02x}", i);
        assert!(db.get(key.as_bytes()).unwrap().is_some());
    }

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §5.2, §6
#[test]
fn ok_stress_concurrent_reads_during_growth() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let seed_count = 0x10;
    for i in 0..seed_count {
        let k = format!("seed_{:04x}", i);
        db.set(k.as_bytes(), b"baseline").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::new();

    for _ in 0..0x02 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        handles.push(thread::spawn(move || {
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

    // Writer thread rapidly creating entries to force multi-page growth
    let db_clone = db.clone();
    let writer_handle = thread::spawn(move || {
        for i in 0..0x80 {
            let k = format!("growth_item_{:04x}", i);
            let v = vec![(i & 0xFF) as u8; 0x30];
            db_clone.set(k.as_bytes(), &v).unwrap();
        }
    });

    writer_handle.join().unwrap();
    running.store(false, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(db.total_entries(), (seed_count + 0x80) as u64);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §6, §7.2, §7.4
#[test]
fn ok_stress_interleaved_kv_and_list_pipeline() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let mut handles = Vec::new();

    // 0x02 KV workers
    for t in 0..0x02 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for i in 0..0x20 {
                let k = format!("kv_pipe_{:02x}_{:04x}", t, i);
                let v = format!("payload_{:04x}", i);
                db_clone.set(k.as_bytes(), v.as_bytes()).unwrap();
            }
        }));
    }

    // 0x02 List queue workers
    for t in 0..0x02 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            let list_name = format!("queue_worker_{:02x}", t);
            let queue = db_clone.list(list_name.as_bytes());
            for i in 0..0x18 {
                let item = format!("item_{:04x}", i);
                queue.append(item.as_bytes()).unwrap();
            }
            assert_eq!(queue.len().unwrap(), 0x18);
            for _ in 0..0x08 {
                assert!(queue.pop().unwrap().is_some());
            }
            assert_eq!(queue.len().unwrap(), 0x10);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // 2 * 0x20 KV entries + 2 lists = 0x40 + 2 = 0x42 entries
    assert_eq!(db.total_entries(), 0x42);

    db.delete().unwrap();
}
