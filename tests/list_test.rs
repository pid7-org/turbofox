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

    // 2. Creating a list then attempting KV get and set must error
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

    // 3. Unified delete succeeds on both KV and list entries
    assert_eq!(db.del(b"kv_key").unwrap(), true);
    assert_eq!(db.del(b"actual_list").unwrap(), true);
    assert_eq!(db.total_entries(), 0);
    assert_eq!(list_key.len().unwrap(), 0);
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

    // Unified delete works identically through db.del
    assert_eq!(db.del(b"to_delete").unwrap(), true);
    assert_eq!(db.total_entries(), 0);
    assert_eq!(list.len().unwrap(), 0);
    assert_eq!(list.pop().unwrap(), None);

    // Deleting again returns false
    assert_eq!(db.del(b"to_delete").unwrap(), false);

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

#[test]
fn ok_turbofox_list_grow_and_reduction_recycles_without_page_growth() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("list_recycle.db");
    let cfg = TurboCfg::new(&db_path);
    let db = TurboFox::create(cfg).unwrap();
    let list = db.list(b"recycle_list");

    // File initially formatted to 8 pages (0x8000 bytes)
    let versioned_file = dir.path().join("list_recycle_1.db");
    let init_len = std::fs::metadata(&versioned_file).unwrap().len();
    assert_eq!(init_len, 0x8000);

    // Repeat grow and shrink across 8 AdjArrayList nodes (2 blocks) 0x0A times
    let cycles = 0x0A;
    let batch_items = 0x18;

    for c in 0..cycles {
        for i in 0..batch_items {
            let item = format!("c{:02x}_i{:02x}", c, i);
            list.append(item.as_bytes()).unwrap();
        }

        assert_eq!(list.len().unwrap(), batch_items);

        // Pop down to 3 items (leaving only head array occupied)
        for _ in 0..(batch_items - 3) {
            assert!(list.pop().unwrap().is_some());
        }
        assert_eq!(list.len().unwrap(), 3);

        // Verify remaining 3 items
        for i in 0..3 {
            let expected = format!("c{:02x}_i{:02x}", c, i);
            assert_eq!(list.get_at(i).unwrap(), Some(expected.into_bytes()));
        }

        // Pop final 3 items
        for _ in 0..3 {
            assert!(list.pop().unwrap().is_some());
        }
        assert_eq!(list.len().unwrap(), 0);
        assert_eq!(list.pop().unwrap(), None);

        // Flush dirty pages to verify file size on storage media
        db.sync().unwrap();
        let cur_len = std::fs::metadata(&versioned_file).unwrap().len();
        assert_eq!(cur_len, 0x8000);
    }

    // Delete list completely and verify final zero-growth state
    assert_eq!(db.del(b"recycle_list").unwrap(), true);
    assert_eq!(db.total_entries(), 0);

    db.sync().unwrap();
    let final_len = std::fs::metadata(&versioned_file).unwrap().len();
    assert_eq!(final_len, 0x8000);
}
