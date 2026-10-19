use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

#[test]
fn ok_stress_concurrent_mixed_set_update_get() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("mixed_stress.db"));
    let db = TurboFox::create(cfg).unwrap();

    let num_shared = 0x10;
    for i in 0..num_shared {
        let key = format!("shared_key_{:04x}", i);
        let val = vec![(i & 0xFF) as u8; 0x20];
        db.set(key.as_bytes(), &val).unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::new();

    // 0x04 writer threads inserting unique keys and triggering growth
    for t in 0..0x04 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for i in 0..0x30 {
                let key = format!("writer_{:02x}_key_{:04x}", t, i);
                let val = vec![((t * 0x10 + i) & 0xFF) as u8; 0x40];
                db_clone.set(key.as_bytes(), &val).unwrap();
            }
        }));
    }

    // 0x04 updater threads mutating shared keys with varying sizes
    for t in 0..0x04 {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for round in 0..0x18 {
                let key_idx = (t + round) % num_shared;
                let key = format!("shared_key_{:04x}", key_idx);
                let size = if round % 2 == 0 { 0x10 } else { 0x60 };
                let val = vec![((round + t) & 0xFF) as u8; size];
                db_clone.set(key.as_bytes(), &val).unwrap();
            }
        }));
    }

    // 0x04 reader threads continuously querying shared and writer keys
    for _ in 0..0x04 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        handles.push(thread::spawn(move || {
            let mut round: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let key_idx = round % num_shared;
                let key = format!("shared_key_{:04x}", key_idx);
                if let Ok(Some(val)) = db_clone.get(key.as_bytes()) {
                    assert!(!val.is_empty());
                }
                round = round.wrapping_add(1);
            }
        }));
    }

    // Wait for writers and updaters to finish
    for h in handles.drain(0..0x08) {
        h.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }

    // Verify all writer keys
    for t in 0..0x04 {
        for i in 0..0x30 {
            let key = format!("writer_{:02x}_key_{:04x}", t, i);
            let expected = vec![((t * 0x10 + i) & 0xFF) as u8; 0x40];
            assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
        }
    }

    db.delete().unwrap();
}

#[test]
fn ok_stress_expansion_during_concurrent_reads() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("read_expansion.db"));
    let db = TurboFox::create(cfg).unwrap();

    let seed_count = 0x20;
    for i in 0..seed_count {
        let key = format!("seed_key_{:04x}", i);
        let val = vec![(i & 0xFF) as u8; 0x30];
        db.set(key.as_bytes(), &val).unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut reader_handles = Vec::new();

    // 0x06 reader threads verifying seed keys continuously
    for _ in 0..0x06 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        reader_handles.push(thread::spawn(move || {
            while running_clone.load(Ordering::Relaxed) {
                for i in 0..seed_count {
                    let key = format!("seed_key_{:04x}", i);
                    let expected = vec![(i & 0xFF) as u8; 0x30];
                    assert_eq!(db_clone.get(key.as_bytes()).unwrap(), Some(expected));
                }
            }
        }));
    }

    // 0x04 writer threads inserting large batches triggering multiple DataBuf growths
    let mut writer_handles = Vec::new();
    for t in 0..0x04 {
        let db_clone = db.clone();
        writer_handles.push(thread::spawn(move || {
            for i in 0..0x28 {
                let key = format!("growth_t{:02x}_k{:04x}", t, i);
                let val = vec![((t ^ i) & 0xFF) as u8; 0x70];
                db_clone.set(key.as_bytes(), &val).unwrap();
            }
        }));
    }

    for wh in writer_handles {
        wh.join().unwrap();
    }

    running.store(false, Ordering::Relaxed);
    for rh in reader_handles {
        rh.join().unwrap();
    }

    db.delete().unwrap();
}

#[test]
fn ok_stress_bucket_split_under_heavy_concurrency() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("bucket_split.db"));
    let db = TurboFox::create(cfg).unwrap();

    // 0x08 threads concurrently inserting keys to force bucket splits
    let num_threads = 0x08;
    let items_per_thread = 0x60;
    let total_expected = num_threads * items_per_thread;

    let mut handles = Vec::new();
    for t in 0..num_threads {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for i in 0..items_per_thread {
                let key = format!("split_t{:02x}_item_{:04x}", t, i);
                let val = format!("val_{:02x}_{:04x}", t, i);
                db_clone.set(key.as_bytes(), val.as_bytes()).unwrap();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(db.total_entries(), total_expected as u64);

    for t in 0..num_threads {
        for i in 0..items_per_thread {
            let key = format!("split_t{:02x}_item_{:04x}", t, i);
            let val = format!("val_{:02x}_{:04x}", t, i);
            assert_eq!(db.get(key.as_bytes()).unwrap(), Some(val.into_bytes()));
        }
    }

    db.delete().unwrap();
}

#[test]
fn ok_stress_hotspot_key_contention() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("hotspot.db"));
    let db = TurboFox::create(cfg).unwrap();

    let num_hotkeys = 0x04;
    for k in 0..num_hotkeys {
        let key = format!("hotkey_{:02x}", k);
        db.set(key.as_bytes(), b"baseline").unwrap();
    }

    let num_threads = 0x08;
    let iterations = 0x20;
    let mut handles = Vec::new();

    for t in 0..num_threads {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            for iter in 0..iterations {
                let key_idx = (t + iter) % num_hotkeys;
                let key = format!("hotkey_{:02x}", key_idx);
                let size = 0x10 + ((t * iter) % 0x50);
                let val = vec![((t + iter) & 0xFF) as u8; size];
                db_clone.set(key.as_bytes(), &val).unwrap();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(db.total_entries(), num_hotkeys as u64);

    for k in 0..num_hotkeys {
        let key = format!("hotkey_{:02x}", k);
        let val = db.get(key.as_bytes()).unwrap();
        assert!(val.is_some());
    }

    db.delete().unwrap();
}

#[test]
fn ok_stress_concurrent_mixed_set_del_get() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("mixed_set_del_get.db"));
    let db = TurboFox::create(cfg).unwrap();

    let key_space: usize = 0x20;
    for i in 0..key_space {
        let key = format!("dynamic_k_{:04x}", i);
        let val = vec![(i & 0xFF) as u8; 0x20];
        db.set(key.as_bytes(), &val).unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::new();

    // 0x03 mutators continuously writing keys
    for t in 0..0x03 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = (t * 0x07 + iter) % key_space;
                let key = format!("dynamic_k_{:04x}", idx);
                let val = vec![((t + iter) & 0xFF) as u8; 0x18];
                let _ = db_clone.set(key.as_bytes(), &val);
                iter = iter.wrapping_add(1);
            }
        }));
    }

    // 0x03 removers continuously deleting keys
    for t in 0..0x03 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = (t * 0x0B + iter) % key_space;
                let key = format!("dynamic_k_{:04x}", idx);
                let _ = db_clone.del(key.as_bytes());
                iter = iter.wrapping_add(1);
            }
        }));
    }

    // 0x03 readers continuously querying keys
    for _ in 0..0x03 {
        let db_clone = db.clone();
        let running_clone = running.clone();
        handles.push(thread::spawn(move || {
            let mut iter: usize = 0;
            while running_clone.load(Ordering::Relaxed) {
                let idx = iter % key_space;
                let key = format!("dynamic_k_{:04x}", idx);
                let _ = db_clone.get(key.as_bytes());
                iter = iter.wrapping_add(1);
            }
        }));
    }

    thread::sleep(std::time::Duration::from_millis(0x32));
    running.store(false, Ordering::Relaxed);

    for h in handles {
        h.join().unwrap();
    }

    // Verify consistency: total_entries matches actual occupied entries found by get
    let mut actual_found: u64 = 0;
    for i in 0..key_space {
        let key = format!("dynamic_k_{:04x}", i);
        if let Ok(Some(_)) = db.get(key.as_bytes()) {
            actual_found += 1;
        }
    }
    assert_eq!(db.total_entries(), actual_found);

    db.delete().unwrap();
}

#[test]
fn ok_stress_dynamic_multithread_kv_lifecycle_validation() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0x04)
        .max(0x02);
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("dyn_kv_val.db"));
    let db = TurboFox::create(cfg).unwrap();
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);

    let per_thread_keys: usize = 0x80;
    let num_shared_keys: usize = 0x08;

    for tid in 0..threads {
        let db = db.clone();
        let barrier = barrier.clone();

        handles.push(thread::spawn(move || {
            // Phase 1: concurrent writes for dedicated and shared keys
            for seq in 0..per_thread_keys {
                let key = format!("k_{:02x}_{:04x}", tid, seq);
                let val: Vec<u8> =
                    (0..0x20).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                let ticket = db.set(key.as_bytes(), &val).unwrap();
                if seq % 0x10 == 0 {
                    let _ = ticket.force();
                }
            }

            for s in 0..num_shared_keys {
                let key = format!("shared_k_{:02x}", s);
                let val: Vec<u8> =
                    (0..0x18).map(|i| ((tid ^ s).wrapping_add(i) & 0xFF) as u8).collect();
                let _ = db.set(key.as_bytes(), &val);
            }

            barrier.wait();

            // Phase 2: read validation for self and neighbor threads
            for seq in 0..per_thread_keys {
                let key = format!("k_{:02x}_{:04x}", tid, seq);
                let expected: Vec<u8> =
                    (0..0x20).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
            }

            let neighbor = (tid + 0x01) % threads;
            for seq in 0..per_thread_keys {
                let key = format!("k_{:02x}_{:04x}", neighbor, seq);
                let expected: Vec<u8> =
                    (0..0x20).map(|i| ((neighbor ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
            }

            for s in 0..num_shared_keys {
                let key = format!("shared_k_{:02x}", s);
                let res = db.get(key.as_bytes()).unwrap();
                assert!(res.is_some());
            }

            barrier.wait();

            // Phase 3: delete even keys
            for seq in (0..per_thread_keys).step_by(0x02) {
                let key = format!("k_{:02x}_{:04x}", tid, seq);
                assert!(db.del(key.as_bytes()).unwrap());
            }

            barrier.wait();

            // Phase 4: post-deletion verification
            for seq in 0..per_thread_keys {
                let key = format!("k_{:02x}_{:04x}", tid, seq);
                if seq % 0x02 == 0 {
                    assert_eq!(db.get(key.as_bytes()).unwrap(), None);
                } else {
                    let expected: Vec<u8> =
                        (0..0x20).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                    assert_eq!(db.get(key.as_bytes()).unwrap(), Some(expected));
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify aggregate total entries
    let surviving_per_thread = per_thread_keys / 0x02;
    let expected_total = ((threads * surviving_per_thread) + num_shared_keys) as u64;
    assert_eq!(db.total_entries(), expected_total);

    db.sync().unwrap();
    db.delete().unwrap();
}

#[test]
fn ok_stress_dynamic_multithread_list_lifecycle_validation() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0x04)
        .max(0x02);
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("dyn_list_val.db"));
    let db = TurboFox::create(cfg).unwrap();
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);

    let per_thread_items: usize = 0x40;
    let shared_items_per_thread: usize = 0x10;

    for tid in 0..threads {
        let db = db.clone();
        let barrier = barrier.clone();

        handles.push(thread::spawn(move || {
            let list_key = format!("list_{:02x}", tid);
            let list = db.list(list_key.as_bytes());
            let shared_list = db.list(b"shared_stress_list");

            // Phase 1: append items
            for seq in 0..per_thread_items {
                let item: Vec<u8> =
                    (0..0x18).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                list.append(&item).unwrap();
            }

            for seq in 0..shared_items_per_thread {
                let item: Vec<u8> =
                    (0..0x14).map(|i| ((tid + seq).wrapping_add(i) & 0xFF) as u8).collect();
                shared_list.append(&item).unwrap();
            }

            barrier.wait();

            // Phase 2: read validation
            assert_eq!(list.len().unwrap(), per_thread_items);
            assert!(!list.is_empty().unwrap());

            let first_exp: Vec<u8> =
                (0..0x18).map(|i| (tid.wrapping_add(i) & 0xFF) as u8).collect();
            assert_eq!(list.first().unwrap(), Some(first_exp));

            let last_idx = per_thread_items - 0x01;
            let last_exp: Vec<u8> =
                (0..0x18).map(|i| ((tid ^ last_idx).wrapping_add(i) & 0xFF) as u8).collect();
            assert_eq!(list.last().unwrap(), Some(last_exp));

            for seq in 0..per_thread_items {
                let exp: Vec<u8> =
                    (0..0x18).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                assert_eq!(list.get_at(seq).unwrap(), Some(exp));
            }

            let collected: Vec<Vec<u8>> = list.iter().unwrap().collect();
            assert_eq!(collected.len(), per_thread_items);

            barrier.wait();

            // Phase 3: pop validation in reverse order
            let pop_count = per_thread_items / 0x02;
            for p in 0..pop_count {
                let seq = per_thread_items - 0x01 - p;
                let exp: Vec<u8> =
                    (0..0x18).map(|i| ((tid ^ seq).wrapping_add(i) & 0xFF) as u8).collect();
                assert_eq!(list.pop().unwrap(), Some(exp));
            }
            assert_eq!(list.len().unwrap(), per_thread_items - pop_count);

            barrier.wait();

            // Phase 4: clear validation
            assert!(list.clear().unwrap());
            assert_eq!(list.len().unwrap(), 0x00);
            assert!(list.is_empty().unwrap());
            assert_eq!(list.pop().unwrap(), None);
            assert_eq!(list.first().unwrap(), None);
            assert_eq!(list.last().unwrap(), None);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Shared list assertions
    let shared_list = db.list(b"shared_stress_list");
    let total_shared = threads * shared_items_per_thread;
    assert_eq!(shared_list.len().unwrap(), total_shared);
    assert!(shared_list.clear().unwrap());
    assert_eq!(shared_list.len().unwrap(), 0x00);
    drop(shared_list);

    db.sync().unwrap();
    db.delete().unwrap();
}

#[test]
fn ok_stress_dynamic_multithread_mixed_all_ops_hammer() {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0x04)
        .max(0x02);
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("dyn_hammer.db"));
    let db = TurboFox::create(cfg).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::with_capacity(threads);

    for tid in 0..threads {
        let db = db.clone();
        let running = running.clone();

        handles.push(thread::spawn(move || {
            let mut step: usize = 0x00;
            let list = db.list(format!("hammer_lst_{:02x}", tid).as_bytes());

            while running.load(Ordering::Relaxed) {
                let k = format!("k_{:02x}_{:02x}", tid, step & 0x1F);
                let v = vec![((tid ^ step) & 0xFF) as u8; 0x28];

                let ticket = db.set(k.as_bytes(), &v).unwrap();
                let _ = db.get(k.as_bytes());

                list.append(&v).unwrap();
                if step & 0x03 == 0x00 {
                    let _ = list.pop();
                }

                if step & 0x07 == 0x00 {
                    let _ = ticket.force();
                }

                if step & 0x0F == 0x00 {
                    let _ = db.del(k.as_bytes());
                }

                step = step.wrapping_add(0x01);
            }
        }));
    }

    thread::sleep(std::time::Duration::from_millis(0x96));
    running.store(false, Ordering::Relaxed);

    for h in handles {
        h.join().unwrap();
    }

    db.sync().unwrap();
    db.delete().unwrap();
}

