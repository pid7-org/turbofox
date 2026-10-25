use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §3.2, §7.1, §7.4
#[test]
fn ok_turbofox_cross_type_safety_and_del_reclaim() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"alpha_queue");
    list.append(b"item_one").unwrap();
    list.append(b"item_two").unwrap();
    assert_eq!(db.total_entries(), 1);

    assert!(matches!(db.get(b"alpha_queue"), Err(TurboError::InvalidType(_))));
    assert!(matches!(db.set(b"alpha_queue", b"payload"), Err(TurboError::InvalidType(_))));
    assert!(matches!(db.contains_key(b"alpha_queue"), Err(TurboError::InvalidType(_))));

    db.set(b"kv_entry", b"raw_bytes").unwrap();
    assert_eq!(db.total_entries(), 2);
    let kv_as_list = db.list(b"kv_entry");
    assert!(matches!(kv_as_list.append(b"sub_item"), Err(TurboError::InvalidType(_))));

    assert!(db.del(b"alpha_queue").unwrap());
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"alpha_queue").unwrap(), None);
    assert!(!db.contains_key(b"alpha_queue").unwrap());

    drop(list);
    drop(kv_as_list);

    db.set(b"alpha_queue", b"now_a_kv").unwrap();
    assert_eq!(db.get(b"alpha_queue").unwrap(), Some(b"now_a_kv".to_vec()));

    db.delete().unwrap();
}
