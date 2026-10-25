use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §8.1
#[test]
fn ok_turbofox_create_open_delete() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());

    let db = TurboFox::create(cfg.clone()).unwrap();
    assert_eq!(db.total_entries(), 0);
    assert!(db.total_pages().unwrap() > 0);
    assert!(db.remaining_bytes().unwrap() > 0);
    drop(db);

    let reopened = TurboFox::open(cfg.clone()).unwrap();
    assert_eq!(reopened.total_entries(), 0);
    reopened.delete().unwrap();
}

/// Ref: `DESIGN.md` §7.1, §7.2, §7.3
#[test]
fn ok_turbofox_kv_crud_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    assert_eq!(db.get(b"missing").unwrap(), None);

    let ticket = db.set(b"dev_id", b"sensor_99").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"dev_id").unwrap(), Some(b"sensor_99".to_vec()));

    db.set(b"dev_id", b"sensor_01").unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"dev_id").unwrap(), Some(b"sensor_01".to_vec()));

    let larger_val = vec![0xAB; 0x0100];
    db.set(b"dev_id", &larger_val).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"dev_id").unwrap(), Some(larger_val));

    assert!(db.del(b"dev_id").unwrap());
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(b"dev_id").unwrap(), None);

    assert!(!db.del(b"dev_id").unwrap());

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn ok_turbofox_persistence_across_reopen() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        for i in 0..0x20 {
            let k = format!("key_{:04}", i).into_bytes();
            let v = format!("val_{:04}", i).into_bytes();
            db.set(&k, &v).unwrap();
        }
        assert_eq!(db.total_entries(), 0x20);
        db.sync().unwrap();
    }

    {
        let reopened = TurboFox::open(cfg.clone()).unwrap();
        assert_eq!(reopened.total_entries(), 0x20);
        for i in 0..0x20 {
            let k = format!("key_{:04}", i).into_bytes();
            let expected_v = format!("val_{:04}", i).into_bytes();
            assert_eq!(reopened.get(&k).unwrap(), Some(expected_v));
        }
        reopened.delete().unwrap();
    }
}

/// Ref: `DESIGN.md` §4.3, §5.4
#[test]
fn ok_turbofox_growth_and_overflow_chaining() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let initial_pages = db.total_pages().unwrap();

    for i in 0..0xC8 {
        let k = format!("heavy_device_record_identity_{:04}", i).into_bytes();
        let v = vec![(i & 0xFF) as u8; 0x40];
        db.set(&k, &v).unwrap();
    }

    assert_eq!(db.total_entries(), 0xC8);
    for i in 0..0xC8 {
        let k = format!("heavy_device_record_identity_{:04}", i).into_bytes();
        let expected_v = vec![(i & 0xFF) as u8; 0x40];
        assert_eq!(db.get(&k).unwrap(), Some(expected_v));
    }

    let grown_pages = db.total_pages().unwrap();
    assert!(grown_pages >= initial_pages);

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.3
#[test]
fn err_turbofox_payload_oversize() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let oversized_key = vec![0x41; 0x0100];
    assert!(matches!(db.set(&oversized_key, b"val"), Err(TurboError::PayloadTooLarge)));

    let oversized_val = vec![0x42; 0x0400];
    assert!(matches!(db.set(b"valid_key", &oversized_val), Err(TurboError::PayloadTooLarge)));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn err_turbofox_delete_busy() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let clone = db.clone();
    assert!(matches!(db.delete(), Err(TurboError::ResourceBusy(_))));

    clone.delete().unwrap();
}
