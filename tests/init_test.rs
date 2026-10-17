//! Integration tests for TurboFox initialization and validation lifecycle

use turbofox::{PAGE_SIZE, TurboCfg, TurboError, TurboFox};

struct TempDb {
    path: std::path::PathBuf,
    versioned: std::path::PathBuf,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let mut path = std::env::temp_dir();
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        path.push(format!("tf_integ_{}_{}_{}.db", name, pid, ts));
        let versioned = TurboCfg::compute_versioned_path(&path, 1);
        if versioned.exists() {
            let _ = std::fs::remove_file(&versioned);
        }
        Self { path, versioned }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        if self.versioned.exists() {
            let _ = std::fs::remove_file(&self.versioned);
        }
    }
}

#[test]
fn ok_turbofox_public_init_and_sync() {
    let temp = TempDb::new("ok_init");
    let cfg = TurboCfg::new(&temp.path);

    {
        let db = TurboFox::new(cfg.clone()).unwrap();
        assert_eq!(db.total_slots(), 0x0100);
        assert!(db.sync().is_ok());
    }

    let meta = std::fs::metadata(&temp.versioned).unwrap();
    assert_eq!(meta.len() as usize, (8 * PAGE_SIZE) as usize);

    let reopened = TurboFox::new(cfg).unwrap();
    assert_eq!(reopened.total_slots(), 0x0100);
}

#[test]
fn err_turbofox_corrupted_file_length() {
    let temp = TempDb::new("err_len");
    std::fs::write(&temp.versioned, [0u8; 0x2000]).unwrap();

    let cfg = TurboCfg::new(&temp.path);
    let err = TurboFox::new(cfg).unwrap_err();
    match err {
        TurboError::InvalidSize { expected, found } => {
            assert_eq!(expected, (8 * PAGE_SIZE) as usize);
            assert_eq!(found, 0x2000);
        }
        other => panic!("unexpected error variant: {:?}", other),
    }
}

#[test]
fn ok_turbofox_with_sync_interval() {
    let temp = TempDb::new("ok_sync_interval");
    let cfg = TurboCfg::new(&temp.path)
        .with_sync_interval(std::time::Duration::from_millis(0x32));

    let db = TurboFox::new(cfg).unwrap();
    assert_eq!(db.total_slots(), 0x0100);
    assert!(db.sync().is_ok());
}

#[test]
fn ok_turbofox_delete() {
    let temp = TempDb::new("ok_delete");
    let cfg = TurboCfg::new(&temp.path);

    let db = TurboFox::new(cfg).unwrap();
    assert!(temp.versioned.exists());

    db.delete().unwrap();
    assert!(!temp.versioned.exists());
}

#[test]
fn err_turbofox_delete_with_active_clone() {
    let temp = TempDb::new("err_delete_clone");
    let cfg = TurboCfg::new(&temp.path);

    let db = TurboFox::new(cfg).unwrap();
    let handle_clone = db.clone();

    let err = db.delete().unwrap_err();
    match err {
        TurboError::ResourceBusy(_) => {}
        other => panic!("unexpected error variant: {:?}", other),
    }

    assert!(temp.versioned.exists());
    handle_clone.delete().unwrap();
    assert!(!temp.versioned.exists());
}
