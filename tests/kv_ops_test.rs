use tempfile::tempdir;
use turbofox::{MAX_KEY_LEN, MAX_VAL_LEN, TurboCfg, TurboError, TurboFox};

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

    // 5: Multi-DBuf payload (8 KiB)
    let large_v = vec![0xAA; 0x2000];
    db.set(b"k_large", &large_v).unwrap();
    assert_eq!(db.get(b"k_large").unwrap(), Some(large_v));
    assert!(db.contains_key(b"k_large").unwrap());
    assert!(db.del(b"k_large").unwrap());

    // 6: Oversized value (> MAX_VAL_LEN)
    let oversize_v = vec![0xAA; MAX_VAL_LEN + 1];
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

/// Ref: `DESIGN.md` §3.3, §3.4, §7.2
#[test]
fn ok_turbofox_kv_spanned_boundary_and_limit_thresholds() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // 1: Exact single-to-multi DBuf threshold (244 B vs 245 B combined)
    let k_thresh = b"k_boundary";
    // 4 B CRC + 10 B key + 230 B val = 244 B (fits in single DBuf, 31 buffers)
    let val_244 = vec![0x11u8; 230];
    db.set(k_thresh, &val_244).unwrap();
    assert_eq!(db.get(k_thresh).unwrap(), Some(val_244));

    // 4 B CRC + 10 B key + 231 B val = 245 B (spans 2 DBufs, 32 buffers)
    let val_245 = vec![0x22u8; 231];
    db.set(k_thresh, &val_245).unwrap();
    assert_eq!(db.get(k_thresh).unwrap(), Some(val_245));
    assert!(db.del(k_thresh).unwrap());

    // 2: Maximum key (255 bytes) with various payload sizes
    let max_k = vec![0x4B; MAX_KEY_LEN];
    let v_small = b"v_small";
    db.set(&max_k, v_small).unwrap();
    assert_eq!(db.get(&max_k).unwrap(), Some(v_small.to_vec()));
    assert!(db.contains_key(&max_k).unwrap());

    // Max key with multi-DBuf value (1 KiB)
    let v_1k = vec![0x33u8; 0x0400];
    db.set(&max_k, &v_1k).unwrap();
    assert_eq!(db.get(&max_k).unwrap(), Some(v_1k));

    // Max key with maximum allowed value (65,535 bytes)
    let max_v = vec![0x77u8; MAX_VAL_LEN];
    db.set(&max_k, &max_v).unwrap();
    assert_eq!(db.get(&max_k).unwrap(), Some(max_v));
    assert!(db.del(&max_k).unwrap());

    // 3: Error on limits: key > MAX_KEY_LEN (256 B) & value > MAX_VAL_LEN (65,536 B)
    let over_k = vec![0x4B; MAX_KEY_LEN + 1];
    assert!(matches!(db.set(&over_k, b"v"), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(&over_k).unwrap(), None);
    assert!(!db.contains_key(&over_k).unwrap());
    assert!(!db.del(&over_k).unwrap());

    let over_v = vec![0x55u8; MAX_VAL_LEN + 1];
    assert!(matches!(db.set(b"k", &over_v), Err(TurboError::PayloadTooLarge)));

    // Combined overshoot: over_k and over_v together
    assert!(matches!(db.set(&over_k, &over_v), Err(TurboError::PayloadTooLarge)));

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.2, §7.3
#[test]
fn ok_turbofox_kv_spanned_lifecycle_and_transitions() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let key = b"lifecycle_key";

    // Step 1: Initial small write (16 B)
    let v_16 = vec![0x10u8; 16];
    db.set(key, &v_16).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(key).unwrap(), Some(v_16));

    // Step 2: Overwrite expanding to multi-DBuf (1024 B)
    let v_1024 = vec![0x20u8; 1024];
    db.set(key, &v_1024).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(key).unwrap(), Some(v_1024));

    // Step 3: Overwrite expanding to larger multi-DBuf (8192 B)
    let v_8k = vec![0x30u8; 8192];
    db.set(key, &v_8k).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(key).unwrap(), Some(v_8k));

    // Step 4: Overwrite contracting to small value (32 B)
    let v_32 = vec![0x40u8; 32];
    db.set(key, &v_32).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(key).unwrap(), Some(v_32.clone()));

    // Step 5: Failed overwrite beyond MAX_VAL_LEN preserves previous value
    let v_invalid = vec![0x99u8; MAX_VAL_LEN + 1];
    assert!(matches!(db.set(key, &v_invalid), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(key).unwrap(), Some(v_32));

    // Step 6: Delete multi-DBuf entry and verify clean removal
    assert!(db.del(key).unwrap());
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(key).unwrap(), None);
    assert!(!db.contains_key(key).unwrap());

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §8
#[test]
fn ok_turbofox_kv_spanned_reopen_and_durability() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().to_path_buf();

    let payloads = vec![
        (b"key_512".to_vec(), vec![0xAAu8; 512]),
        (b"key_2048".to_vec(), vec![0xBBu8; 2048]),
        (b"key_16k".to_vec(), vec![0xCCu8; 16384]),
        (b"key_32k".to_vec(), vec![0xDDu8; 32768]),
    ];

    // Write all entries
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::create(cfg).unwrap();

        for (k, v) in &payloads {
            db.set(k, v).unwrap();
        }
        assert_eq!(db.total_entries(), 4);
        db.sync().unwrap();
    }

    // Reopen and verify byte-for-byte durability
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::open(cfg).unwrap();

        assert_eq!(db.total_entries(), 4);
        for (k, v) in &payloads {
            assert!(db.contains_key(k).unwrap());
            assert_eq!(db.get(k).unwrap(), Some(v.clone()));
        }

        // Delete half
        assert!(db.del(&payloads[0].0).unwrap());
        assert!(db.del(&payloads[2].0).unwrap());
        assert_eq!(db.total_entries(), 2);
        db.sync().unwrap();
    }

    // Reopen again and verify deletions persisted
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::open(cfg).unwrap();

        assert_eq!(db.total_entries(), 2);
        assert_eq!(db.get(&payloads[0].0).unwrap(), None);
        assert_eq!(db.get(&payloads[1].0).unwrap(), Some(payloads[1].1.clone()));
        assert_eq!(db.get(&payloads[2].0).unwrap(), None);
        assert_eq!(db.get(&payloads[3].0).unwrap(), Some(payloads[3].1.clone()));

        db.delete().unwrap();
    }
}

/// Ref: `DESIGN.md` §3.3, §3.4, §7.2
#[test]
fn ok_turbofox_kv_limit_matrix() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let matrix: Vec<(Vec<u8>, Vec<u8>)> = vec![
        // (1 B key, 0 B val) - smallest valid entry
        (vec![0xAA], vec![]),
        // (1 B key, 65,535 B val) - smallest key with max value
        (vec![0xBB], vec![0x12; MAX_VAL_LEN]),
        // (254 B key, 1 B val) - off-by-one below max key
        (vec![0xCC; MAX_KEY_LEN - 1], vec![0x34]),
        // (255 B key, 0 B val) - max key with zero-length value
        (vec![0xDD; MAX_KEY_LEN], vec![]),
        // (255 B key, 65,535 B val) - simultaneous maximum key and value
        (vec![0xEE; MAX_KEY_LEN], vec![0x56; MAX_VAL_LEN]),
    ];

    for (k, v) in &matrix {
        let ticket = db.set(k, v).unwrap();
        assert!(ticket.epoch() > 0);
        assert!(db.contains_key(k).unwrap());
        assert_eq!(db.get(k).unwrap(), Some(v.clone()));

        assert!(db.del(k).unwrap());
        assert!(!db.contains_key(k).unwrap());
        assert_eq!(db.get(k).unwrap(), None);
        assert_eq!(db.total_entries(), 0);
    }

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.3, §7.2, §7.3
#[test]
fn err_turbofox_kv_limit_violations() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // 1: Off-by-one key length (256 B)
    let key_256 = vec![0x41; MAX_KEY_LEN + 1];
    assert!(matches!(db.set(&key_256, b"val"), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(&key_256).unwrap(), None);
    assert!(!db.contains_key(&key_256).unwrap());
    assert!(!db.del(&key_256).unwrap());

    // 2: Off-by-one value length (65,536 B)
    let val_65536 = vec![0x42; MAX_VAL_LEN + 1];
    assert!(matches!(db.set(b"valid_k", &val_65536), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(b"valid_k").unwrap(), None);
    assert!(!db.contains_key(b"valid_k").unwrap());

    // 3: Double overshoot (256 B key + 65,536 B value)
    assert!(matches!(db.set(&key_256, &val_65536), Err(TurboError::PayloadTooLarge)));

    // 4: Gross overshoot (1,024 B key + 131,072 B value)
    let key_gross = vec![0x43; 0x0400];
    let val_gross = vec![0x44; 0x0002_0000];
    assert!(matches!(db.set(&key_gross, &val_gross), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(&key_gross).unwrap(), None);
    assert!(!db.contains_key(&key_gross).unwrap());
    assert!(!db.del(&key_gross).unwrap());

    // 5: Empty key with max value
    let val_max = vec![0x45; MAX_VAL_LEN];
    assert!(matches!(db.set(b"", &val_max), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(b"").unwrap(), None);
    assert!(!db.contains_key(b"").unwrap());
    assert!(!db.del(b"").unwrap());

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §4.3, §7.2
#[test]
fn ok_turbofox_kv_max_key_prefix_collision() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    // 3 keys of 255 bytes sharing first 254 bytes, differing only in the last byte
    let mut k1 = vec![0x50; MAX_KEY_LEN];
    let mut k2 = vec![0x50; MAX_KEY_LEN];
    let mut k3 = vec![0x50; MAX_KEY_LEN];
    k1[MAX_KEY_LEN - 1] = 0x01;
    k2[MAX_KEY_LEN - 1] = 0x02;
    k3[MAX_KEY_LEN - 1] = 0x03;

    let v1 = vec![0xA1; 0x0400]; // 1 KiB
    let v2 = vec![0xA2; 0x1000]; // 4 KiB
    let v3 = vec![0xA3; 0x4000]; // 16 KiB

    db.set(&k1, &v1).unwrap();
    db.set(&k2, &v2).unwrap();
    db.set(&k3, &v3).unwrap();
    assert_eq!(db.total_entries(), 3);

    // Verify all distinct
    assert_eq!(db.get(&k1).unwrap(), Some(v1.clone()));
    assert_eq!(db.get(&k2).unwrap(), Some(v2.clone()));
    assert_eq!(db.get(&k3).unwrap(), Some(v3.clone()));

    // Overwrite middle key without clobbering neighbors
    let v2_new = vec![0xB2; 0x2000]; // 8 KiB
    db.set(&k2, &v2_new).unwrap();
    assert_eq!(db.total_entries(), 3);
    assert_eq!(db.get(&k1).unwrap(), Some(v1));
    assert_eq!(db.get(&k2).unwrap(), Some(v2_new));
    assert_eq!(db.get(&k3).unwrap(), Some(v3.clone()));

    // Delete middle key, verify neighbors remain intact
    assert!(db.del(&k2).unwrap());
    assert_eq!(db.total_entries(), 2);
    assert_eq!(db.get(&k2).unwrap(), None);
    assert!(db.contains_key(&k1).unwrap());
    assert!(db.contains_key(&k3).unwrap());

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §7.2, §7.3
#[test]
fn ok_turbofox_kv_max_payload_transitions() {
    let dir = tempdir().unwrap();
    let cfg = TurboCfg::manual(dir.path());
    let db = TurboFox::create(cfg).unwrap();

    let max_k = vec![0x7A; MAX_KEY_LEN];
    let max_v = vec![0x7B; MAX_VAL_LEN];

    // 1: Insert maximum payload pair (255 B key + 65,535 B val)
    db.set(&max_k, &max_v).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(&max_k).unwrap(), Some(max_v.clone()));

    // 2: Overwrite in-place contracting to 1 byte
    let v_tiny = vec![0x09];
    db.set(&max_k, &v_tiny).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(&max_k).unwrap(), Some(v_tiny));

    // 3: Overwrite expanding back to maximum payload
    let max_v2 = vec![0x7C; MAX_VAL_LEN];
    db.set(&max_k, &max_v2).unwrap();
    assert_eq!(db.total_entries(), 1);
    assert_eq!(db.get(&max_k).unwrap(), Some(max_v2.clone()));

    // 4: Failed overwrite beyond limit preserves current value
    let v_oversize = vec![0xFF; MAX_VAL_LEN + 1];
    assert!(matches!(db.set(&max_k, &v_oversize), Err(TurboError::PayloadTooLarge)));
    assert_eq!(db.get(&max_k).unwrap(), Some(max_v2));

    // 5: Clean deletion of max payload entry
    assert!(db.del(&max_k).unwrap());
    assert_eq!(db.total_entries(), 0);
    assert_eq!(db.get(&max_k).unwrap(), None);

    db.delete().unwrap();
}

/// Ref: `DESIGN.md` §3.4, §5.3, §8
#[test]
fn ok_turbofox_kv_max_payload_cluster_durability() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().to_path_buf();

    // 4 entries, each with 255 B key and 65,535 B val (each spans 266 DBuf pools)
    let cluster: Vec<(Vec<u8>, Vec<u8>)> = (0..4)
        .map(|i| {
            let mut k = vec![0x60 + i as u8; MAX_KEY_LEN];
            k[MAX_KEY_LEN - 1] = i as u8;
            let v = vec![0x80 + i as u8; MAX_VAL_LEN];
            (k, v)
        })
        .collect();

    // Initial write and sync
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::create(cfg).unwrap();

        for (k, v) in &cluster {
            db.set(k, v).unwrap();
        }
        assert_eq!(db.total_entries(), 4);
        db.sync().unwrap();
    }

    // Reopen and verify data and CRC32 across all entries
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::open(cfg).unwrap();

        assert_eq!(db.total_entries(), 4);
        for (k, v) in &cluster {
            assert!(db.contains_key(k).unwrap());
            assert_eq!(db.get(k).unwrap(), Some(v.clone()));
        }

        // Delete 2 entries
        assert!(db.del(&cluster[0].0).unwrap());
        assert!(db.del(&cluster[2].0).unwrap());
        assert_eq!(db.total_entries(), 2);
        db.sync().unwrap();
    }

    // Reopen second time: verify persistence and reuse reclaimed space
    {
        let cfg = TurboCfg::manual(&db_path);
        let db = TurboFox::open(cfg).unwrap();

        assert_eq!(db.total_entries(), 2);
        assert_eq!(db.get(&cluster[0].0).unwrap(), None);
        assert_eq!(db.get(&cluster[1].0).unwrap(), Some(cluster[1].1.clone()));
        assert_eq!(db.get(&cluster[2].0).unwrap(), None);
        assert_eq!(db.get(&cluster[3].0).unwrap(), Some(cluster[3].1.clone()));

        // Insert new entry into reclaimed database
        let mut new_k = vec![0x90; MAX_KEY_LEN];
        new_k[0] = 0xFE;
        let new_v = vec![0x95; MAX_VAL_LEN];
        db.set(&new_k, &new_v).unwrap();
        assert_eq!(db.total_entries(), 3);
        assert_eq!(db.get(&new_k).unwrap(), Some(new_v));

        db.delete().unwrap();
    }
}
