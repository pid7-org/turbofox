use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

/// Ref: `DESIGN.md` §8.1
#[test]
fn ok_turbofox_persistence_across_reopen() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        for i in 0..0x10 {
            let k = format!("persist_k_{:02x}", i);
            let v = format!("persist_v_{:02x}", i);
            db.set(k.as_bytes(), v.as_bytes()).unwrap();
        }
        assert_eq!(db.total_entries(), 0x10);
        db.sync().unwrap();
    }

    {
        let db = TurboFox::open(cfg.clone()).unwrap();
        assert_eq!(db.total_entries(), 0x10);

        for i in 0..0x10 {
            let k = format!("persist_k_{:02x}", i);
            let expected_v = format!("persist_v_{:02x}", i);
            assert_eq!(db.get(k.as_bytes()).unwrap(), Some(expected_v.into_bytes()));
        }

        let expanded = vec![0x33; 0x0120];
        db.set(b"persist_k_00", &expanded).unwrap();
        db.set(b"persist_k_new", b"val_new").unwrap();
        assert_eq!(db.total_entries(), 0x11);
        db.sync().unwrap();
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_entries(), 0x11);
    assert_eq!(reopened.get(b"persist_k_00").unwrap(), Some(vec![0x33; 0x0120]));
    assert_eq!(reopened.get(b"persist_k_new").unwrap(), Some(b"val_new".to_vec()));

    reopened.delete().unwrap();
}

/// Ref: `DESIGN.md` §8.1
#[test]
fn ok_turbofox_ticket_force_durability() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());

    {
        let db = TurboFox::create(cfg.clone()).unwrap();
        let ticket = db.set(b"forced_key", b"forced_value").unwrap();
        assert!(ticket.force().is_ok());
        assert!(ticket.is_durable());
    }

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.get(b"forced_key").unwrap(), Some(b"forced_value".to_vec()));
    reopened.delete().unwrap();
}
