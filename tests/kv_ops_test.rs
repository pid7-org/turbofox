use tempfile::tempdir;
use turbofox::{MAX_KEY_LEN, TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §7.1, §7.2, §7.3
#[test]
fn ok_turbofox_kv_crud_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    assert_eq!(db.get(b"sensor_x").unwrap(), None);
    assert!(!db.contains_key(b"sensor_x").unwrap());

    // 1: Initial insert
    let ticket = db.set(b"sensor_x", b"temp:23.5").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(db.total_entries(), 1);
    assert!(db.contains_key(b"sensor_x").unwrap());
    assert_eq!(db.get(b"sensor_x").unwrap(), Some(b"temp:23.5".to_vec()));

    // 2: In-place update with smaller value
    db.set(b"sensor_x", b"temp:0").unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"sensor_x").unwrap(), Some(b"temp:0".to_vec()));

    // 3: Expansion overwrite with larger value
    let larger_val = vec![0xEE; 0xA0];
    db.set(b"sensor_x", &larger_val).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"sensor_x").unwrap(), Some(larger_val));

    // 4: Delete key
    assert!(db.del(b"sensor_x").unwrap());
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(b"sensor_x").unwrap(), None);
    assert!(!db.contains_key(b"sensor_x").unwrap());
    assert!(!db.del(b"sensor_x").unwrap());

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.3, §7.2
#[test]
fn ok_turbofox_kv_boundary_values_and_keys() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // 1: Zero-length value
    db.set(b"empty_key", b"").unwrap();
    assert_eq!(db.get(b"empty_key").unwrap(), Some(vec![]));
    assert!(db.contains_key(b"empty_key").unwrap());

    // 2: Large key fitting in DBuf pool
    let max_k = vec![0x4B; 0xD0];
    db.set(&max_k, b"max_key_val").unwrap();
    assert_eq!(db.get(&max_k).unwrap(), Some(b"max_key_val".to_vec()));

    // 3: Oversized key (> 255 bytes)
    let oversize_k = vec![0x4B; MAX_KEY_LEN + 1];
    assert!(matches!(db.set(&oversize_k, b"v"), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(&oversize_k).unwrap(), None);
    assert!(!db.contains_key(&oversize_k).unwrap());
    assert!(!db.del(&oversize_k).unwrap());

    // 4: Empty key behavior
    assert_eq!(db.get(b"").unwrap(), None);
    assert!(!db.contains_key(b"").unwrap());
    assert!(!db.del(b"").unwrap());
    assert!(matches!(db.set(b"", b"val"), Err(TurboError::PayloadTooLarge)));

    // 5: Oversized value (> DBuf capacity)
    let oversize_v = vec![0xAA; 0x0400];
    assert!(matches!(db.set(b"k", &oversize_v), Err(TurboError::PayloadTooLarge)));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §4.3, §7.3
#[test]
fn ok_turbofox_kv_tombstone_slot_reclaim() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    db.set(b"tomb_k", b"v1").unwrap();
    assert_eq!(db.total_entries(), 1);

    assert!(db.del(b"tomb_k").unwrap());
    assert_eq!(db.total_entries(), 0);

    // Re-inserting the same key claims tombstone
    db.set(b"tomb_k", b"v2").unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(b"tomb_k").unwrap(), Some(b"v2".to_vec()));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.2, §7.1
#[test]
fn err_turbofox_kv_cross_type_safety() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"list_key");
    list.append(b"item").unwrap();

    assert!(matches!(db.get(b"list_key"), Err(TurboError::InvalidType(_))));
    assert!(matches!(db.set(b"list_key", b"v"), Err(TurboError::InvalidType(_))));
    assert!(matches!(db.contains_key(b"list_key"), Err(TurboError::InvalidType(_))));

    drop(list);
    db.delete().unwrap();
}
