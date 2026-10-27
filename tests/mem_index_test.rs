use std::sync::Arc;
use std::thread;
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

#[test]
fn ok_mem_index_basic_lifecycle() {
    let dir = tempdir().expect("tempdir failed");
    let cfg = TurboCfg::manual(dir.path()).with_mem_index(true);
    let db = TurboFox::create(cfg).expect("create failed");

    for i in 0x00..0x64 {
        let key = format!("k_{:04x}", i);
        let val = format!("v_{:04x}", i);
        db.set(key.as_bytes(), val.as_bytes()).expect("set failed");
    }

    for i in 0x00..0x64 {
        let key = format!("k_{:04x}", i);
        let val = format!("v_{:04x}", i);
        let res = db.get(key.as_bytes()).expect("get failed");
        assert_eq!(res, Some(val.into_bytes()));
        assert!(db.contains_key(key.as_bytes()).expect("contains_key failed"));
    }

    assert_eq!(db.get(b"missing_key").expect("get failed"), None);
    assert!(!db.contains_key(b"missing_key").expect("contains_key failed"));

    for i in 0x00..0x32 {
        let key = format!("k_{:04x}", i);
        assert!(db.del(key.as_bytes()).expect("del failed"));
        assert_eq!(db.get(key.as_bytes()).expect("get failed"), None);
        assert!(!db.contains_key(key.as_bytes()).expect("contains_key failed"));
    }

    for i in 0x32..0x64 {
        let key = format!("k_{:04x}", i);
        let val = format!("v_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).expect("get failed"), Some(val.into_bytes()));
    }

    db.sync().expect("sync failed");
}

#[test]
fn ok_mem_index_reopen_and_persistence() {
    let dir = tempdir().expect("tempdir failed");
    let path = dir.path().to_path_buf();

    {
        let cfg = TurboCfg::manual(&path).with_mem_index(true);
        let db = TurboFox::create(cfg).expect("create failed");

        for i in 0x00..0x80 {
            let key = format!("entry_{:04x}", i);
            let val = format!("payload_{:04x}", i);
            db.set(key.as_bytes(), val.as_bytes()).expect("set failed");
        }

        for i in (0x00..0x20).step_by(2) {
            let key = format!("entry_{:04x}", i);
            assert!(db.del(key.as_bytes()).expect("del failed"));
        }

        db.sync().expect("sync failed");
    }

    {
        let cfg = TurboCfg::manual(&path).with_mem_index(true);
        let db = TurboFox::open(cfg).expect("open failed");

        for i in 0x00..0x80 {
            let key = format!("entry_{:04x}", i);
            let val = format!("payload_{:04x}", i);
            if i < 0x20 && i % 2 == 0 {
                assert_eq!(db.get(key.as_bytes()).expect("get failed"), None);
            } else {
                assert_eq!(db.get(key.as_bytes()).expect("get failed"), Some(val.into_bytes()));
            }
        }
    }
}

#[test]
fn ok_mem_index_concurrent_readers_and_writers() {
    let dir = tempdir().expect("tempdir failed");
    let cfg = TurboCfg::manual(dir.path()).with_mem_index(true);
    let db = Arc::new(TurboFox::create(cfg).expect("create failed"));

    for i in 0x00..0x64 {
        let key = format!("initial_{:04x}", i);
        let val = format!("val_{:04x}", i);
        db.set(key.as_bytes(), val.as_bytes()).expect("set failed");
    }

    let thread_count = 0x04;
    let ops_per_thread = 0xC8;
    let mut handles = Vec::with_capacity(thread_count);

    for tid in 0x00..thread_count {
        let db_clone = Arc::clone(&db);
        handles.push(thread::spawn(move || {
            for i in 0x00..ops_per_thread {
                let key = format!("thread_{:02x}_key_{:04x}", tid, i);
                let val = format!("thread_{:02x}_val_{:04x}", tid, i);

                db_clone.set(key.as_bytes(), val.as_bytes()).expect("set failed");

                let read_val = db_clone.get(key.as_bytes()).expect("get failed");
                assert_eq!(read_val, Some(val.into_bytes()));

                if i % 3 == 0 {
                    let _ = db_clone.del(key.as_bytes()).expect("del failed");
                    assert_eq!(db_clone.get(key.as_bytes()).expect("get failed"), None);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("worker thread panicked");
    }

    for i in 0x00..0x64 {
        let key = format!("initial_{:04x}", i);
        let val = format!("val_{:04x}", i);
        assert_eq!(db.get(key.as_bytes()).expect("get failed"), Some(val.into_bytes()));
    }
}

#[test]
fn ok_mem_index_parity_with_disabled_mode() {
    let dir_a = tempdir().expect("tempdir failed");
    let dir_b = tempdir().expect("tempdir failed");

    let db_simd = TurboFox::create(TurboCfg::manual(dir_a.path()).with_mem_index(true))
        .expect("create simd failed");
    let db_scalar = TurboFox::create(TurboCfg::manual(dir_b.path()).with_mem_index(false))
        .expect("create scalar failed");

    for i in 0x00..0xC8 {
        let key = format!("parity_key_{:04x}", i);
        let val = format!("parity_val_{:04x}", i);
        db_simd.set(key.as_bytes(), val.as_bytes()).expect("set failed");
        db_scalar.set(key.as_bytes(), val.as_bytes()).expect("set failed");
    }

    for i in 0x00..0xC8 {
        let key = format!("parity_key_{:04x}", i);
        assert_eq!(
            db_simd.get(key.as_bytes()).expect("get failed"),
            db_scalar.get(key.as_bytes()).expect("get failed"),
        );
        assert_eq!(
            db_simd.contains_key(key.as_bytes()).expect("contains_key failed"),
            db_scalar.contains_key(key.as_bytes()).expect("contains_key failed"),
        );
    }

    for i in (0x00..0xC8).step_by(2) {
        let key = format!("parity_key_{:04x}", i);
        let del_simd = db_simd.del(key.as_bytes()).expect("del failed");
        let del_scalar = db_scalar.del(key.as_bytes()).expect("del failed");
        assert_eq!(del_simd, del_scalar);
    }

    for i in 0x00..0xC8 {
        let key = format!("parity_key_{:04x}", i);
        assert_eq!(
            db_simd.get(key.as_bytes()).expect("get failed"),
            db_scalar.get(key.as_bytes()).expect("get failed"),
        );
    }
}
