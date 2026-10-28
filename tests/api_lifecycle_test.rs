use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;
use tempfile::tempdir;
use turbofox::{SyncMode, TurboCfg, TurboError, TurboFox};

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
    assert!(db.remaining_pages().unwrap() > 0);
    assert!(db.remaining_bytes().unwrap() > 0);

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
