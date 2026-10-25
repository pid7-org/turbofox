use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

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
fn err_turbofox_open_corrupted_magic() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

    let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"CORR").unwrap();
    drop(file);

    let err = TurboFox::open(cfg).unwrap_err();
    assert!(matches!(err, TurboError::InvalidMagic));
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn err_turbofox_open_version_mismatch() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

    let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&0x0000_0009u32.to_le_bytes()).unwrap();
    drop(file);

    let err = TurboFox::open(cfg).unwrap_err();
    assert!(matches!(err, TurboError::VersionMismatch { expected: 2, found: 9 }));
}

/// Ref: `DESIGN.md` §3.1, §8.1
#[test]
fn err_turbofox_open_corrupted_total_slots() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();

    let mut file = OpenOptions::new().write(true).open(&file_path).unwrap();
    file.seek(SeekFrom::Start(0x10)).unwrap();
    file.write_all(&0x0000_07FFu32.to_le_bytes()).unwrap();
    drop(file);

    let err = TurboFox::open(cfg).unwrap_err();
    assert!(matches!(err, TurboError::CorruptedHeader(_)));
}
