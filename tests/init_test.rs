use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

const PAGE_SIZE: usize = 0x1000;

#[test]
fn ok_turbofox_public_init_and_sync() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("init.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        assert!(db.sync().is_ok());
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();
    let meta = std::fs::metadata(file_path).unwrap();
    assert_eq!(meta.len() as usize, (8 * PAGE_SIZE) as usize);

    let reopened = TurboFox::open(cfg).unwrap();
    reopened.delete().unwrap();
}

#[test]
fn err_turbofox_create_already_exists() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("exists.db"));

    let db = TurboFox::create(cfg.clone()).unwrap();
    let err = TurboFox::create(cfg).unwrap_err();
    match err {
        TurboError::Storage { .. } => {}
        other => panic!("unexpected error variant: {:?}", other),
    }
    db.delete().unwrap();
}

#[test]
fn err_turbofox_open_not_exists() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("nonexistent.db"));

    let err = TurboFox::open(cfg).unwrap_err();
    match err {
        TurboError::Storage { .. } => {}
        other => panic!("unexpected error variant: {:?}", other),
    }
}

#[test]
fn ok_turbofox_multiple_version_instances() {
    let dir = tempdir().unwrap();
    let base = dir.path().join("multi.db");
    let cfg1 = TurboCfg::new(&base).with_version(1);
    let cfg2 = TurboCfg::new(&base).with_version(2);

    let db1 = TurboFox::create(cfg1).unwrap();
    let db2 = TurboFox::create(cfg2).unwrap();

    db1.set(b"key_one", b"val_one").unwrap();
    db2.set(b"key_two", b"val_two").unwrap();

    assert_eq!(db1.get(b"key_one").unwrap(), Some(b"val_one".to_vec()));
    assert_eq!(db1.get(b"key_two").unwrap(), None);

    assert_eq!(db2.get(b"key_two").unwrap(), Some(b"val_two".to_vec()));
    assert_eq!(db2.get(b"key_one").unwrap(), None);

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);

    db1.delete().unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    db2.delete().unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn err_turbofox_corrupted_file_length() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("corrupt.db"));

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        db.sync().unwrap();
    }

    let file_path =
        std::fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok()).next().unwrap().path();
    let file = std::fs::OpenOptions::new().write(true).open(&file_path).unwrap();
    file.set_len(0x2000).unwrap();
    drop(file);

    let err = TurboFox::open(cfg).unwrap_err();
    match err {
        TurboError::InvalidSize { expected, found } => {
            assert_eq!(expected, (8 * PAGE_SIZE) as usize);
            assert_eq!(found, 0x2000);
        }
        TurboError::Storage { .. } => {}
        other => panic!("unexpected error variant: {:?}", other),
    }
}

#[test]
fn ok_turbofox_with_sync_interval() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("sync.db"))
        .with_sync_interval(std::time::Duration::from_millis(0x32));

    let db = TurboFox::create(cfg).unwrap();
    assert!(db.sync().is_ok());
    db.delete().unwrap();
}

#[test]
fn ok_turbofox_delete() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("delete.db"));

    let db = TurboFox::create(cfg).unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    db.delete().unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn err_turbofox_delete_with_active_clone() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path().join("delete_clone.db"));

    let db = TurboFox::create(cfg).unwrap();
    let handle_clone = db.clone();

    let err = db.delete().unwrap_err();
    match err {
        TurboError::ResourceBusy(_) => {}
        other => panic!("unexpected error variant: {:?}", other),
    }

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    handle_clone.delete().unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
