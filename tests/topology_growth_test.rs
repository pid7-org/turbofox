use tempfile::tempdir;
use turbofox::{TurboCfg, TurboFox};

/// Ref: `DESIGN.md` §4.3, §5.3, §5.4
#[test]
fn ok_turbofox_bucket_overflow_chaining() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let initial_pages = db.total_pages().unwrap();
    let count = 0xC8;

    // Insert 200 keys; root bucket holds 64 slots, forcing multiple bucket expansions
    for i in 0..count {
        let k = format!("overflow_route_key_{:04}", i).into_bytes();
        let v = vec![(i & 0xFF) as u8; 0x40];
        db.set(&k, &v).unwrap();
    }

    assert_eq!(db.total_entries(), count);
    for i in 0..count {
        let k = format!("overflow_route_key_{:04}", i).into_bytes();
        let expected = vec![(i & 0xFF) as u8; 0x40];
        assert_eq!(db.get(&k).unwrap(), Some(expected));
    }

    let grown_pages = db.total_pages().unwrap();
    assert!(grown_pages >= initial_pages);

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.3, §5.3
#[test]
fn ok_turbofox_databuf_cursor_hint_and_wraparound() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg.clone()).unwrap();

    // Initial pool 0x10 holds 63 buffers of 16 B; 4-byte key + 8-byte val consumes 1 buffer
    for i in 0..0x3F {
        let k = format!("k{:03}", i).into_bytes();
        db.set(&k, b"12345678").unwrap();
    }

    // Next insert overflows pool 0x10 and advances hint to pool 0x18
    let overflow_key = b"k063";
    db.set(overflow_key, b"12345678").unwrap();

    // Fill pool 0x18 (remaining 62 entries)
    for i in 0x40..0x7E {
        let k = format!("k{:03}", i).into_bytes();
        db.set(&k, b"12345678").unwrap();
    }

    // Next insert exhausts pool 0x18, triggers expansion under growth_mutex
    let expand_key = b"k126";
    db.set(expand_key, b"12345678").unwrap();

    // Free a slot in pool 0x10
    assert!(db.del(b"k000").unwrap());

    // Fill remaining buffers in expanded pool
    for i in 0x7F..0xBD {
        let k = format!("k{:03}", i).into_bytes();
        db.set(&k, b"12345678").unwrap();
    }

    // Allocation wraps around to head and harvests slot freed in pool 0x10
    db.set(b"h000", b"12345678").unwrap();

    // Reopening database resets in-memory hint safely to DATABUF_HEAD_SLOT
    db.sync().unwrap();
    drop(db);
    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.get(b"h000").unwrap(), Some(b"12345678".to_vec()));

    reopened.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §5.3
#[test]
fn ok_turbofox_adjarr_block_expansion() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let list = db.list(b"growth_queue");
    // Initial 7 blocks accommodate 42 arrays (each array holds 4 items = 168 items if chained)
    // Appending 0xC0 items exercises multi-array chaining and block expansions
    let count = 0xC0;

    for i in 0..count {
        let item = format!("item_payload_{:04}", i).into_bytes();
        list.append(&item).unwrap();
    }

    assert_eq!(list.len().unwrap(), count);
    for i in 0..count {
        let expected = format!("item_payload_{:04}", i).into_bytes();
        assert_eq!(list.get_at(i).unwrap(), Some(expected));
    }

    for i in (0..count).rev() {
        let expected = format!("item_payload_{:04}", i).into_bytes();
        assert_eq!(list.pop().unwrap(), Some(expected));
    }
    assert_eq!(list.len().unwrap(), 0);

    drop(list);
    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §5.1, §5.4
#[test]
fn ok_turbofox_physical_os_page_growth() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg.clone()).unwrap();

    let initial_pages = db.total_pages().unwrap();
    let initial_rem_bytes = db.remaining_bytes().unwrap();

    // Ingest heavy payloads to consume slots and trigger OS page growth
    for i in 0..0x80 {
        let k = format!("page_grow_{:04}", i).into_bytes();
        let v = vec![0x3C; 0x0180];
        db.set(&k, &v).unwrap();
    }

    let grown_pages = db.total_pages().unwrap();
    assert!(grown_pages >= initial_pages);
    assert_eq!(db.total_entries(), 0x80);

    db.sync().unwrap();
    drop(db);

    let reopened = TurboFox::open(cfg).unwrap();
    assert_eq!(reopened.total_pages().unwrap(), grown_pages);
    assert_eq!(reopened.total_entries(), 0x80);
    assert!(reopened.remaining_bytes().unwrap() <= initial_rem_bytes);

    reopened.delete().unwrap();
}
