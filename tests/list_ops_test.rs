use tempfile::tempdir;
use turbofox::{TurboCfg, TurboError, TurboFox};

/// Ref: `DESIGN.md` §7.4
#[test]
fn ok_turbofox_list_append_pop_lifecycle() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"packet_stream");
    assert_eq!(list.key(), b"packet_stream");

    // Empty list invariants
    assert_eq!(list.len().unwrap(), 0);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), None);
    assert_eq!(list.last().unwrap(), None);
    assert_eq!(list.pop().unwrap(), None);
    assert_eq!(list.get_at(0).unwrap(), None);
    assert_eq!(list.get_at(0x0100).unwrap(), None);

    // Appending elements
    let ticket = list.append(b"head_pkt").unwrap();
    assert!(ticket.epoch() > 0);
    assert_eq!(list.len().unwrap(), 1);
    assert!(!list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), Some(b"head_pkt".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"head_pkt".to_vec()));
    assert_eq!(list.get_at(0).unwrap(), Some(b"head_pkt".to_vec()));

    list.append(b"mid_pkt").unwrap();
    list.append(b"tail_pkt").unwrap();
    assert_eq!(list.len().unwrap(), 3);
    assert_eq!(list.first().unwrap(), Some(b"head_pkt".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"tail_pkt".to_vec()));
    assert_eq!(list.get_at(1).unwrap(), Some(b"mid_pkt".to_vec()));

    // LIFO popping
    assert_eq!(list.pop().unwrap(), Some(b"tail_pkt".to_vec()));
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.pop().unwrap(), Some(b"mid_pkt".to_vec()));
    assert_eq!(list.pop().unwrap(), Some(b"head_pkt".to_vec()));
    assert_eq!(list.pop().unwrap(), None);
    assert_eq!(list.len().unwrap(), 0);

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_multi_block_chaining_and_iter() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"chained_events");
    let count = 0x20;

    for i in 0..count {
        let payload = format!("event_id_{:04}", i).into_bytes();
        list.append(&payload).unwrap();
    }
    assert_eq!(list.len().unwrap(), count);

    for i in 0..count {
        let expected = format!("event_id_{:04}", i).into_bytes();
        assert_eq!(list.get_at(i).unwrap(), Some(expected));
    }

    let items: Vec<Vec<u8>> = list.iter().unwrap().collect();
    assert_eq!(items.len(), count);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item, &format!("event_id_{:04}", i).into_bytes());
    }

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §7.4
#[test]
fn ok_turbofox_list_clear_and_type_check() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"clearable_queue");
    list.append(b"item_0").unwrap();
    list.append(b"item_1").unwrap();

    assert!(list.clear().unwrap());
    assert_eq!(list.len().unwrap(), 0);
    assert!(!list.clear().unwrap());

    db.set(b"kv_entry", b"payload").unwrap();
    let invalid_list = db.list(b"kv_entry");
    assert!(matches!(invalid_list.append(b"fail"), Err(TurboError::InvalidType(_))));

    drop(list);
    drop(invalid_list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §7.3, §7.4
#[test]
fn ok_turbofox_list_del_via_engine() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"engine_del_list");
    list.append(b"elem").unwrap();
    drop(list);

    assert_eq!(db.total_entries(), 1);
    assert!(db.del(b"engine_del_list").unwrap());
    assert_eq!(db.total_entries(), 0);
    assert!(!db.del(b"engine_del_list").unwrap());

    db.delete().unwrap();
}
