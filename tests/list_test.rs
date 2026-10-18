//! Integration tests for TurboFox list operations
//!
//! Ref: `DESIGN.md` §3.6, §5.4, §7.4

use std::sync::Arc;
use tempfile::NamedTempFile;
use turbofox::{TurboCfg, TurboError, TurboFox};

struct TempDb {
    _file: NamedTempFile,
    cfg: TurboCfg,
}

impl TempDb {
    fn new(prefix: &str) -> Self {
        let file = NamedTempFile::with_prefix(prefix).unwrap();
        let path = file.path().to_path_buf();
        let cfg = TurboCfg::new(&path);
        Self { _file: file, cfg }
    }
}

#[test]
fn ok_turbofox_list_basic_append_pop() {
    let temp = TempDb::new("ok_list_basic");
    let db = TurboFox::create(temp.cfg).unwrap();
    let list = db.list(b"test_list");

    assert_eq!(list.len().unwrap(), 0);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), None);
    assert_eq!(list.last().unwrap(), None);

    list.append(b"item_1").unwrap();
    assert_eq!(list.len().unwrap(), 1);
    assert!(!list.is_empty().unwrap());
    assert_eq!(list.first().unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.get_at(0).unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.get_at(1).unwrap(), None);

    list.append(b"item_2").unwrap();
    list.append(b"item_3").unwrap();
    assert_eq!(list.len().unwrap(), 3);
    assert_eq!(list.first().unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"item_3".to_vec()));
    assert_eq!(list.get_at(0).unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.get_at(1).unwrap(), Some(b"item_2".to_vec()));
    assert_eq!(list.get_at(2).unwrap(), Some(b"item_3".to_vec()));
    assert_eq!(list.get_at(3).unwrap(), None);

    assert_eq!(list.pop().unwrap(), Some(b"item_3".to_vec()));
    assert_eq!(list.len().unwrap(), 2);
    assert_eq!(list.last().unwrap(), Some(b"item_2".to_vec()));

    assert_eq!(list.pop().unwrap(), Some(b"item_2".to_vec()));
    assert_eq!(list.len().unwrap(), 1);
    assert_eq!(list.last().unwrap(), Some(b"item_1".to_vec()));

    assert_eq!(list.pop().unwrap(), Some(b"item_1".to_vec()));
    assert_eq!(list.len().unwrap(), 0);
    assert!(list.is_empty().unwrap());
    assert_eq!(list.pop().unwrap(), None);
}

#[test]
fn ok_turbofox_list_spanned_nodes_and_iter() {
    let temp = TempDb::new("ok_list_spanned");
    let db = TurboFox::create(temp.cfg).unwrap();
    let list = db.list(b"chained_list");

    // Insert 0x0A items (spanning across 4 AdjArrayList nodes of 3 items each)
    for i in 0..0x0A {
        let val = format!("val_{:02x}", i);
        list.append(val.as_bytes()).unwrap();
    }

    assert_eq!(list.len().unwrap(), 0x0A);
    assert_eq!(list.first().unwrap(), Some(b"val_00".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"val_09".to_vec()));

    // Verify indexed get_at across chained nodes
    for i in 0..0x0A {
        let expected = format!("val_{:02x}", i);
        assert_eq!(list.get_at(i).unwrap(), Some(expected.into_bytes()));
    }
    assert_eq!(list.get_at(0x0A).unwrap(), None);

    // Verify iterator
    let items: Vec<Vec<u8>> = list.iter().unwrap().collect();
    assert_eq!(items.len(), 0x0A);
    for (i, item) in items.iter().enumerate() {
        let expected = format!("val_{:02x}", i);
        assert_eq!(item, expected.as_bytes());
    }

    // Pop all items in LIFO order
    for i in (0..0x0A).rev() {
        let expected = format!("val_{:02x}", i);
        assert_eq!(list.pop().unwrap(), Some(expected.into_bytes()));
    }
    assert_eq!(list.len().unwrap(), 0);
    assert_eq!(list.pop().unwrap(), None);
}

#[test]
fn ok_turbofox_list_type_isolation_and_invariants() {
    let temp = TempDb::new("ok_list_type_iso");
    let db = TurboFox::create(temp.cfg).unwrap();

    // 1. Setting KV key then attempting list operations must error
    db.set(b"kv_key", b"some_val").unwrap();
    let kv_as_list = db.list(b"kv_key");
    assert!(matches!(
        kv_as_list.append(b"item"),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.pop(),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.len(),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.first(),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.last(),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.get_at(0),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        kv_as_list.iter(),
        Err(TurboError::InvalidType(_))
    ));

    // 2. Creating a list then attempting KV operations must error
    let list_key = db.list(b"actual_list");
    list_key.append(b"first_item").unwrap();
    assert!(matches!(
        db.get(b"actual_list"),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        db.set(b"actual_list", b"new_val"),
        Err(TurboError::InvalidType(_))
    ));
    assert!(matches!(
        db.del(b"actual_list"),
        Err(TurboError::InvalidType(_))
    ));
}

#[test]
fn ok_turbofox_list_delete_and_recreation() {
    let temp = TempDb::new("ok_list_del");
    let db = TurboFox::create(temp.cfg).unwrap();
    let list = db.list(b"to_delete");

    for i in 0..0x08 {
        list.append(format!("item_{:02x}", i).as_bytes()).unwrap();
    }
    assert_eq!(list.len().unwrap(), 0x08);
    assert_eq!(db.total_entries(), 1);

    // Delete list
    assert_eq!(db.list_del(b"to_delete").unwrap(), true);
    assert_eq!(db.total_entries(), 0);
    assert_eq!(list.len().unwrap(), 0);
    assert_eq!(list.pop().unwrap(), None);

    // Deleting again returns false
    assert_eq!(db.list_del(b"to_delete").unwrap(), false);

    // Recreate with new items
    list.append(b"resurrected_item").unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(list.len().unwrap(), 1);
    assert_eq!(list.first().unwrap(), Some(b"resurrected_item".to_vec()));
}

#[test]
fn ok_turbofox_list_persistence_reopen() {
    let temp = TempDb::new("ok_list_persist");
    {
        let db = TurboFox::create(temp.cfg.clone()).unwrap();
        let list1 = db.list(b"persist_list_1");
        let list2 = db.list(b"persist_list_2");

        list1.append(b"l1_item1").unwrap();
        list1.append(b"l1_item2").unwrap();
        list2.append(b"l2_item1").unwrap();

        db.sync().unwrap();
    }

    let reopened = TurboFox::open(temp.cfg).unwrap();
    assert_eq!(reopened.total_entries(), 2);

    let list1 = reopened.list(b"persist_list_1");
    let list2 = reopened.list(b"persist_list_2");

    assert_eq!(list1.len().unwrap(), 2);
    assert_eq!(list1.get_at(0).unwrap(), Some(b"l1_item1".to_vec()));
    assert_eq!(list1.get_at(1).unwrap(), Some(b"l1_item2".to_vec()));

    assert_eq!(list2.len().unwrap(), 1);
    assert_eq!(list2.get_at(0).unwrap(), Some(b"l2_item1".to_vec()));
}

#[test]
fn ok_turbofox_list_growth_expansion() {
    let temp = TempDb::new("ok_list_growth");
    let db = TurboFox::create(temp.cfg).unwrap();

    // Initial allocation holds 0x4E arrays (0x0D blocks of 6 arrays)
    // Appending 0x0100 items requires 0x56 arrays (> 0x4E), triggering grow_adjarr
    let list = db.list(b"massive_list");
    for i in 0..0x0100 {
        let val = format!("val_{:04x}", i);
        list.append(val.as_bytes()).unwrap();
    }

    assert_eq!(list.len().unwrap(), 0x0100);
    assert_eq!(list.first().unwrap(), Some(b"val_0000".to_vec()));
    assert_eq!(list.last().unwrap(), Some(b"val_00ff".to_vec()));

    for i in 0..0x0100 {
        let expected = format!("val_{:04x}", i);
        assert_eq!(list.get_at(i).unwrap(), Some(expected.into_bytes()));
    }
}

#[test]
fn ok_turbofox_concurrent_list_append() {
    let temp = TempDb::new("ok_list_concurrent");
    let db = Arc::new(TurboFox::create(temp.cfg).unwrap());

    let mut handles = Vec::new();
    let thread_count = 4;
    let items_per_thread = 0x10;

    for t in 0..thread_count {
        let db_clone = Arc::clone(&db);
        let handle = std::thread::spawn(move || {
            let list = db_clone.list(b"shared_list");
            for i in 0..items_per_thread {
                let payload = format!("t{:02x}_i{:02x}", t, i);
                list.append(payload.as_bytes()).unwrap();
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    let list = db.list(b"shared_list");
    assert_eq!(list.len().unwrap(), thread_count * items_per_thread);
}
