use std::thread;
use std::time::Duration;
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

#[test]
fn ok_turbofox_persistence_explicit_sync() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("explicit_sync.db"));
    let db = TurboFox::create(cfg.clone()).unwrap();

    for i in 0..0x0A {
        let k = format!("sync_key_{:02x}", i);
        let v = format!("sync_val_{:02x}", i);
        db.set(k.as_bytes(), v.as_bytes()).unwrap();
    }
    assert_eq!(db.total_entries(), 0x0A);

    db.sync().unwrap();
    drop(db);

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 0x0A);
    for i in 0..0x0A {
        let k = format!("sync_key_{:02x}", i);
        let v = format!("sync_val_{:02x}", i);
        assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(v.into_bytes()));
    }

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_persistence_drop_flush() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("drop_flush.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        for i in 0..0x08 {
            let k = format!("drop_k_{:02x}", i);
            let v = format!("drop_v_{:02x}", i);
            db.set(k.as_bytes(), v.as_bytes()).unwrap();
        }
        // Explicit sync prior to handle destruction ensures metadata durable
        db.sync().unwrap();
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 0x08);
    for i in 0..0x08 {
        let k = format!("drop_k_{:02x}", i);
        let v = format!("drop_v_{:02x}", i);
        assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(v.into_bytes()));
    }

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_persistence_open_write_drop_reopen_read() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("open_write_drop.db"));

    // 1: Create and populate baseline
    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.set(b"seed_alpha", b"init_alpha").unwrap();
        db.set(b"seed_beta", b"init_beta").unwrap();
        db.sync().unwrap();
    }

    // 2: Open existing, write new entries and update existing
    {
        let db = TurboFox::open(cfg.clone()).unwrap();
        assert_eq!(db.total_entries(), 2);

        // Update seed_alpha with expansion payload
        let expanded = vec![0x55; 0x0110];
        db.set(b"seed_alpha", &expanded).unwrap();

        // Add new key
        db.set(b"seed_gamma", b"init_gamma").unwrap();
        assert_eq!(db.total_entries(), 3);

        db.sync().unwrap();
    }

    // 3: Reopen and verify persistence of modified topology
    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 3);

    let expected_expanded = vec![0x55; 0x0110];
    assert_eq!(reopened.get(b"seed_alpha").unwrap(), Some(expected_expanded));
    assert_eq!(reopened.get(b"seed_beta").unwrap(), Some(b"init_beta".to_vec()));
    assert_eq!(reopened.get(b"seed_gamma").unwrap(), Some(b"init_gamma".to_vec()));

    reopened.delete().unwrap();
}

#[test]
fn ok_turbofox_persistence_background_sync_interval() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("bg_sync.db"))
        .with_sync_interval(Duration::from_millis(0x32));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        for i in 0..0x0A {
            let k = format!("bg_key_{:02x}", i);
            let v = format!("bg_val_{:02x}", i);
            db.set(k.as_bytes(), v.as_bytes()).unwrap();
        }

        // Allow background thread in MemMap to trigger flush
        thread::sleep(Duration::from_millis(0x96));
        db.sync().unwrap();
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 0x0A);
    for i in 0..0x0A {
        let k = format!("bg_key_{:02x}", i);
        let v = format!("bg_val_{:02x}", i);
        assert_eq!(reopened.get(k.as_bytes()).unwrap(), Some(v.into_bytes()));
    }

    reopened.delete().unwrap();
}
