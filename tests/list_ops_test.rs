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

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_spanned_boundary_and_limit_thresholds() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // Key thresholds: 243 B (single pool) vs 244 B and 255 B (spanned across 2 pools)
    let key_single = vec![0x31; 0xF3];
    let key_spanned_32 = vec![0x32; 0xF4];
    let key_spanned_max = vec![0x33; 0xFF];

    // Item thresholds: 242 B (single pool) vs 243 B (spanned into 2nd pool)
    let item_single = vec![0x41; 0xF2];
    let item_spanned = vec![0x42; 0xF3];

    let l1 = db.list(&key_single);
    l1.append(&item_single).unwrap();
    l1.append(&item_spanned).unwrap();
    assert_eq!(l1.len().unwrap(), 2);
    assert_eq!(l1.first().unwrap(), Some(item_single.clone()));
    assert_eq!(l1.last().unwrap(), Some(item_spanned.clone()));

    let l2 = db.list(&key_spanned_32);
    l2.append(&item_spanned).unwrap();
    assert_eq!(l2.len().unwrap(), 1);
    assert_eq!(l2.first().unwrap(), Some(item_spanned.clone()));

    let l3 = db.list(&key_spanned_max);
    l3.append(&item_single).unwrap();
    l3.append(&item_spanned).unwrap();
    assert_eq!(l3.len().unwrap(), 2);
    assert_eq!(l3.get_at(0).unwrap(), Some(item_single));
    assert_eq!(l3.get_at(1).unwrap(), Some(item_spanned));

    drop(l1);
    drop(l2);
    drop(l3);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_spanned_lifecycle_and_popping() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let long_key = vec![0x4B; 0xFF];
    let list = db.list(&long_key);

    let item_1k = vec![0xAA; 0x0400];
    let item_4k = vec![0xBB; 0x1000];
    let item_max = vec![0xCC; 0xFFFF];

    list.append(&item_1k).unwrap();
    list.append(&item_4k).unwrap();
    list.append(&item_max).unwrap();

    assert_eq!(list.len().unwrap(), 3);
    assert_eq!(list.first().unwrap(), Some(item_1k.clone()));
    assert_eq!(list.last().unwrap(), Some(item_max.clone()));

    // LIFO popping
    assert_eq!(list.pop().unwrap(), Some(item_max));
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.last().unwrap(), Some(item_4k.clone()));

    assert_eq!(list.pop().unwrap(), Some(item_4k));
    assert_eq!(list.len().unwrap(), 1);
    assert_eq!(list.last().unwrap(), Some(item_1k.clone()));

    // Append new spanned item to verify slot reuse
    let item_2k = vec![0xDD; 0x0800];
    list.append(&item_2k).unwrap();
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.last().unwrap(), Some(item_2k));

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_spanned_traversal_and_iteration() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"iter_spanned_queue");
    let count = 0x0C;
    let mut expected_items = Vec::with_capacity(count);

    for i in 0..count {
        let mut payload = vec![(i as u8) + 0x10; 0x0200];
        payload[0] = (i >> 8) as u8;
        payload[1] = (i & 0xFF) as u8;
        list.append(&payload).unwrap();
        expected_items.push(payload);
    }

    assert_eq!(list.len().unwrap(), count);

    for (i, expected) in expected_items.iter().enumerate() {
        assert_eq!(list.get_at(i).unwrap(), Some(expected.clone()));
    }

    let collected: Vec<Vec<u8>> = list.iter().unwrap().collect();
    assert_eq!(collected.len(), count);
    for (i, item) in collected.iter().enumerate() {
        assert_eq!(item, &expected_items[i]);
    }

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_max_payload_and_limit_matrix() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let max_key = vec![0x5A; 0xFF];
    let max_item1 = vec![0x6A; 0xFFFF];
    let max_item2 = vec![0x7B; 0xFFFF];

    let list = db.list(&max_key);
    assert_eq!(list.key(), &max_key[..]);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.len().unwrap(), 0);
    assert_eq!(list.first().unwrap(), None);
    assert_eq!(list.last().unwrap(), None);
    assert_eq!(list.get_at(0).unwrap(), None);

    list.append(&max_item1).unwrap();
    assert_eq!(list.len().unwrap(), 1);
    assert!(!list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), Some(max_item1.clone()));
    assert_eq!(list.last().unwrap(), Some(max_item1.clone()));
    assert_eq!(list.get_at(0).unwrap(), Some(max_item1.clone()));

    list.append(&max_item2).unwrap();
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.first().unwrap(), Some(max_item1.clone()));
    assert_eq!(list.last().unwrap(), Some(max_item2.clone()));
    assert_eq!(list.get_at(0).unwrap(), Some(max_item1.clone()));
    assert_eq!(list.get_at(1).unwrap(), Some(max_item2.clone()));
    assert_eq!(list.get_at(2).unwrap(), None);

    let collected: Vec<Vec<u8>> = list.iter().unwrap().collect();
    assert_eq!(collected, vec![max_item1.clone(), max_item2.clone()]);

    assert_eq!(list.pop().unwrap(), Some(max_item2));
    assert_eq!(list.len().unwrap(), 1);
    assert_eq!(list.last().unwrap(), Some(max_item1));

    assert!(list.clear().unwrap());
    assert_eq!(list.len().unwrap(), 0);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), None);
    assert_eq!(list.last().unwrap(), None);

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §7.4
#[test]
fn err_turbofox_list_limit_violations() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // 0-length key rejection
    let empty_list = db.list(b"");
    assert!(matches!(empty_list.append(b"val"), Err(TurboError::PayloadTooLarge)));
    assert_eq!(empty_list.pop().unwrap(), None);
    assert_eq!(empty_list.first().unwrap(), None);

    // Oversized key: 256 B > MAX_KEY_LEN
    let long_key = vec![0x41; 0x0100];
    let long_key_list = db.list(&long_key);
    assert!(matches!(long_key_list.append(b"val"), Err(TurboError::PayloadTooLarge)));

    // Oversized item: 65,536 B > MAX_VAL_LEN
    let normal_list = db.list(b"normal_key");
    let oversized_item = vec![0x42; 0x0001_0000];
    assert!(matches!(normal_list.append(&oversized_item), Err(TurboError::PayloadTooLarge)));

    // Cross-type safety
    db.set(b"type_kv_key", b"scalar_val").unwrap();
    let mismatched_list = db.list(b"type_kv_key");
    assert!(matches!(mismatched_list.append(b"v"), Err(TurboError::InvalidType(_))));

    let list_key = db.list(b"type_list_key");
    list_key.append(b"elem").unwrap();
    assert!(matches!(db.set(b"type_list_key", b"v"), Err(TurboError::InvalidType(_))));

    drop(empty_list);
    drop(long_key_list);
    drop(normal_list);
    drop(mismatched_list);
    drop(list_key);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.4
#[test]
fn ok_turbofox_list_spanned_reopen_and_durability() {
    let dir = tempdir().unwrap();
    let path = dir.path().to_path_buf();

    let max_key = vec![0x4B; 0xFF];
    let item_1k = vec![0x11; 0x0400];
    let item_8k = vec![0x22; 0x2000];
    let item_max = vec![0x33; 0xFFFF];

    {
        let cfg = TurboCfg::manual(&path);
        let db = TurboFox::create(cfg).unwrap();
        let list = db.list(&max_key);

        let t1 = list.append(&item_1k).unwrap();
        let t2 = list.append(&item_8k).unwrap();
        let t3 = list.append(&item_max).unwrap();

        t1.force().unwrap();
        t2.force().unwrap();
        t3.force().unwrap();
    }

    {
        let cfg = TurboCfg::manual(&path);
        let db = TurboFox::open(cfg).unwrap();
        let list = db.list(&max_key);

        assert_eq!(list.len().unwrap(), 3);
        assert_eq!(list.get_at(0).unwrap(), Some(item_1k.clone()));
        assert_eq!(list.get_at(1).unwrap(), Some(item_8k));
        assert_eq!(list.get_at(2).unwrap(), Some(item_max.clone()));

        assert_eq!(list.pop().unwrap(), Some(item_max));
        assert_eq!(list.len().unwrap(), 2);

        let item_4k = vec![0x44; 0x1000];
        let t4 = list.append(&item_4k).unwrap();
        t4.force().unwrap();
    }

    {
        let cfg = TurboCfg::manual(&path);
        let db = TurboFox::open(cfg).unwrap();
        let list = db.list(&max_key);

        assert_eq!(list.len().unwrap(), 3);
        assert_eq!(list.first().unwrap(), Some(item_1k));
        assert_eq!(list.last().unwrap(), Some(vec![0x44; 0x1000]));

        drop(list);
        db.delete().unwrap();
    }
}
