use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;
use tempfile::tempdir;
use turbofox::{CapacityHint, SyncMode, TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn ok_turbofox_lifecycle_configs_and_metrics() {
    let dir = tempdir().unwrap();

    // 1: Manual sync mode lifecycle
    let manual_cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(manual_cfg.clone()).unwrap();

    assert!(db.is_empty());
    assert_eq!(db.total_entries(), 0);
    assert!(db.total_pages().unwrap() > 0);
    assert!(db.remaining_pages().is_ok());
    assert!(db.remaining_bytes().unwrap() > 0);
    assert!(db.resident_bytes_in_memory().unwrap() > 0);

    db.set(b"key_a", b"val_a").unwrap();
    assert!(!db.is_empty());
    assert_eq!(db.total_entries(), 1);
    db.sync().unwrap();
    drop(db);

    // 2: Periodic sync mode reopen
    let periodic_cfg = TurboCfg::new(dir.path(), SyncMode::Periodic(Duration::from_millis(0x32)));
    let reopened = TurboFox::open(periodic_cfg).unwrap();
    assert_eq!(reopened.total_entries(), 1);
    assert!(!reopened.is_empty());
    assert_eq!(reopened.get(b"key_a").unwrap(), Some(b"val_a".to_vec()));

    reopened.delete().unwrap();
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn err_turbofox_delete_busy_guards() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let clone = db.clone();
    assert!(matches!(db.delete(), Err(TurboError::ResourceBusy(_))));

    drop(clone);
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn err_turbofox_create_already_exists() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    let db = TurboFox::create(cfg.clone()).unwrap();
    let err = TurboFox::create(cfg).unwrap_err();
    assert!(matches!(err, TurboError::Storage { .. }));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn err_turbofox_open_corrupted_headers_matrix() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

    // 1: Corrupted magic header
    {
        let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"CORR").unwrap();
    }
    assert!(matches!(TurboFox::open(cfg.clone()).unwrap_err(), TurboError::InvalidMagic));

    // Restore magic, corrupt version
    {
        let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"TBFX").unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&0x0000_0009u32.to_le_bytes()).unwrap();
    }
    assert!(matches!(
        TurboFox::open(cfg.clone()).unwrap_err(),
        TurboError::VersionMismatch { expected: 3, found: 9 }
    ));

    // Restore version, corrupt total_slots divergence
    {
        let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&0x0000_0003u32.to_le_bytes()).unwrap();
        file.seek(SeekFrom::Start(0x10)).unwrap();
        file.write_all(&0x0000_07FFu32.to_le_bytes()).unwrap();
    }
    assert!(matches!(TurboFox::open(cfg.clone()).unwrap_err(), TurboError::CorruptedHeader(_)));
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn ok_turbofox_open_or_create_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    // 1: First call creates fresh database
    {
        let db = TurboFox::open_or_create(cfg.clone()).unwrap();
        assert_eq!(db.total_entries(), 0);
        db.set(b"service:state", b"running").unwrap();
        db.sync().unwrap();
    }

    // 2: Subsequent call reopens existing database
    {
        let reopened = TurboFox::open_or_create(cfg.clone()).unwrap();
        assert_eq!(reopened.total_entries(), 1);
        assert_eq!(reopened.get(b"service:state").unwrap(), Some(b"running".to_vec()));
        reopened.delete().unwrap();
    }
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn err_turbofox_open_or_create_corrupt_fails_safely() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::open_or_create(cfg.clone()).unwrap();
        db.set(b"critical_key", b"payload").unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

    // Corrupt magic header in existing file
    {
        let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(b"CORR").unwrap();
    }

    // open_or_create must return error and not overwrite the corrupt file
    assert!(matches!(TurboFox::open_or_create(cfg.clone()).unwrap_err(), TurboError::InvalidMagic));
}

/// Ref: `DESIGN.md` §5.4, §8.1
#[test]
fn ok_turbofox_capacity_hint_preallocation_and_reopen() {
    let dir = tempdir().unwrap();
    let hint = CapacityHint::new().with_kv(0x03E8, 0x10, 0x20).with_list(0x64, 0x05, 0x20);

    let cfg = TurboCfg::manual(dir.path()).with_capacity_hint(hint);
    let db = TurboFox::create(cfg.clone()).unwrap();

    let initial_pages = db.total_pages().unwrap();
    assert!(initial_pages >= 0x04);
    let initial_remaining = db.remaining_bytes().unwrap();
    assert!(initial_remaining > 0x4000);

    // Verify operations work on pre-allocated storage
    db.set(b"hint_key_1", b"hint_val_1").unwrap();
    assert_eq!(db.get(b"hint_key_1").unwrap(), Some(b"hint_val_1".to_vec()));

    let list = db.list(b"hint_list_1");
    list.append(b"item_1").unwrap();
    assert_eq!(list.len().unwrap(), 0x01);
    drop(list);

    db.sync().unwrap();
    drop(db);

    // Reopen and assert structure preserved
    let reopened = TurboFox::open(cfg.clone()).unwrap();
    assert_eq!(reopened.total_pages().unwrap(), initial_pages);
    assert_eq!(reopened.get(b"hint_key_1").unwrap(), Some(b"hint_val_1".to_vec()));
    assert_eq!(reopened.list(b"hint_list_1").len().unwrap(), 0x01);

    reopened.delete().unwrap();
}
