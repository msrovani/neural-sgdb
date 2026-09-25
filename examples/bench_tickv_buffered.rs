//! TickvFile: legado (open+flush por op) vs buffered (handle persistente).
//! Run: `cargo run --release --example bench_tickv_buffered`

use std::time::Instant;
use neural_sgdb::{Sgdb, TickvFile};

fn main() {
    const N: usize = 5_000;
    for mode in ["legacy", "buffered"] {
        let dir = std::env::temp_dir().join("neural_sgdb_bench_tickv_buf");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("tickv_{mode}.db"));
        let _ = std::fs::remove_file(&path);
        let storage = match mode {
            "buffered" => TickvFile::open_buffered(&path).expect("open buffered"),
            _ => TickvFile::open(&path).expect("open legacy"),
        };
        let mut db = Sgdb::open(storage).expect("open sgdb");
        let t = Instant::now();
        for i in 0..N {
            db.remember_text_with(&format!("k/{i:06}"), &format!("buffered bench doc {i} about memory states"), Default::default())
                .unwrap();
        }
        let writes = t.elapsed();
        let t = Instant::now();
        for _i in 0..500 {
            let _ = db.recall_lexical("buffered bench memory", 5).unwrap();
        }
        let reads = t.elapsed();
        let t = Instant::now();
        for i in 0..1_000 {
            let _ = db.delete(&format!("md/L3/k/{i:06}")).unwrap();
        }
        let dels = t.elapsed();
        println!(
            "{mode:<9} write {:.1} µs/op | recall {:.1} µs/op | delete {:.1} µs/op",
            writes.as_secs_f64() * 1e6 / N as f64,
            reads.as_secs_f64() * 1e6 / 500.0,
            dels.as_secs_f64() * 1e6 / 1000.0,
        );
        let _ = std::fs::remove_file(&path);
    }
}
