//! WASM/OPFS backend demo (v1.1.15 §4 P1): ciclo export → import sem browser.
//!
//! No browser o host persiste `to_bytes()` num arquivo OPFS/IndexedDB e
//! restaura com `from_bytes()` na abertura — o lib continua zero-dep.
//!
//! ```sh
//! cargo run --example wasm_backend
//! ```

use neural_sgdb::{SnapshotStorage, Sgdb, Storage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Sgdb sobre backend snapshotável (uso normal)
    let back = SnapshotStorage::new();
    let mut db = Sgdb::open(back)?;
    db.remember_text_with(
        "hello",
        "ola mundo wasm",
        neural_sgdb::RememberOptions::default(),
    )?;
    db.set_ttl("md/L3/hello", 999)?;
    db.set_event("md/L3/hello", "demo/wasm", 0)?;
    let hits = db.recall_lexical("ola wasm", 5)?;
    assert!(hits.iter().any(|h| h.key == "md/L3/hello"));

    // 2. ciclo canônico backend→bytes→backend (o que o OPFS faria no host)
    let mut s = SnapshotStorage::new();
    s.put(b"md/L3/hello", b"ola mundo wasm")?;
    let snap = s.to_bytes();
    let mut r = SnapshotStorage::from_bytes(&snap)?;
    assert_eq!(r.get(b"md/L3/hello")?.unwrap(), b"ola mundo wasm");
    assert!(SnapshotStorage::from_bytes(&snap[..snap.len() / 2]).is_err());

    println!(
        "wasm_backend: OK (snapshot {} bytes, {} hits)",
        snap.len(),
        hits.len()
    );
    Ok(())
}
