use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §7.4
#[test]
fn ok_turbofox_list_append_pop_basic() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"sensor_queue");
    assert_eq!(list.len().unwrap(), 0);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.pop().unwrap(), None);

    let ticket = list.append(b"packet_0").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(list.len().unwrap(), 1);
    assert!(!list.is_empty().unwrap());

    list.append(b"packet_1").unwrap();
    list.append(b"packet_2").unwrap();
    assert_eq!(list.len().unwrap(), 3);

    assert_eq!(list.first().unwrap(), Some(b"packet_0".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"packet_2".to_vec()));
    assert_eq!(list.get_at(1).unwrap(), Some(b"packet_1".to_vec()));

    assert_eq!(list.pop().unwrap(), Some(b"packet_2".to_vec()));
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.pop().unwrap(), Some(b"packet_1".to_vec()));
    assert_eq!(list.pop().unwrap(), Some(b"packet_0".to_vec()));
    assert_eq!(list.pop().unwrap(), None);
    assert_eq!(list.len().unwrap(), 0);

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_multi_block_chaining() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"chained_log");

    for i in 0..0x19 {
        let payload = format!("log_event_{:03}", i).into_bytes();
        list.append(&payload).unwrap();
    }

    assert_eq!(list.len().unwrap(), 0x19);

    for i in 0..0x19 {
        let expected = format!("log_event_{:03}", i).into_bytes();
        assert_eq!(list.get_at(i).unwrap(), Some(expected));
    }

    let items: Vec<Vec<u8>> = list.iter().unwrap().collect();
    assert_eq!(items.len(), 0x19);
    for (i, it) in items.iter().enumerate() {
        assert_eq!(it, &format!("log_event_{:03}", i).into_bytes());
    }

    for i in (0..0x19).rev() {
        let expected = format!("log_event_{:03}", i).into_bytes();
        assert_eq!(list.pop().unwrap(), Some(expected));
    }
    assert_eq!(list.len().unwrap(), 0);

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §7.4
#[test]
fn ok_turbofox_list_clear_and_type_check() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::new(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"my_list");
    list.append(b"item_a").unwrap();
    list.append(b"item_b").unwrap();

    assert!(matches!(db.get(b"my_list"), Err(TurboError::InvalidType(_))));
    assert!(matches!(db.set(b"my_list", b"val"), Err(TurboError::InvalidType(_))));

    assert!(list.clear().unwrap());
    assert_eq!(list.len().unwrap(), 0);

    db.set(b"my_list", b"kv_value").unwrap();
    assert_eq!(db.get(b"my_list").unwrap(), Some(b"kv_value".to_vec()));

    let kv_as_list = db.list(b"my_list");
    assert!(matches!(kv_as_list.append(b"sub_item"), Err(TurboError::InvalidType(_))));

    drop(list);
    drop(kv_as_list);
    db.delete().unwrap();
}
