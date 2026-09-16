//! Telepatia em **dois ficheiros DB** (FileStorage) — prova multi-nó real.
//!
//! Diferente de `p2p_telepathy` (InMemory): aqui A e B têm paths distintos,
//! sync CRDT + reopen a partir do disco. Modelo Cursor multi-agente com DBs
//! isolados (não o ficheiro MCP partilhado).
//!
//! ```text
//! cargo run --release --example telepathy_two_db --features p2p
//! ```

use neural_sgdb::{
    CrdtMemorySync, FileStorage, MemoryLayer, MemoryState, MergeVerdict, Sgdb, SgdbError,
    Transport,
};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Pipe {
    buf: Vec<(u8, u64)>,
}

impl Pipe {
    fn take(&mut self) -> Vec<(u8, u64)> {
        core::mem::take(&mut self.buf)
    }
    fn from(pkts: Vec<(u8, u64)>) -> Self {
        Self { buf: pkts }
    }
}

impl Transport for Pipe {
    fn send_crdt(&mut self, node: u8, v: u64) -> Result<(), SgdbError> {
        self.buf.push((node, v));
        Ok(())
    }
    fn send_delta(&mut self, node: u8, v: u64, _p: &[u8]) -> Result<(), SgdbError> {
        self.buf.push((node, v));
        Ok(())
    }
    fn recv_crdt(&mut self) -> Vec<(u8, u64)> {
        self.take()
    }
}

struct Node {
    path: PathBuf,
    db: Sgdb,
    crdt: CrdtMemorySync,
    out: Pipe,
}

impl Node {
    fn open(id: u8, path: PathBuf) -> Result<Self, SgdbError> {
        let db = Sgdb::open(
            FileStorage::open(&path).map_err(|_| SgdbError::Storage("file open failed"))?,
        )?;
        Ok(Self {
            path,
            db,
            crdt: CrdtMemorySync::new(id),
            out: Pipe::default(),
        })
    }

    fn reopen(id: u8, path: PathBuf) -> Result<Self, SgdbError> {
        Self::open(id, path)
    }

    fn remember(&mut self, key: &str, text: &str, emb: &[f32]) -> Result<(), SgdbError> {
        self.db.remember_semantic(key, text, emb)?;
        self.crdt.record_change();
        Ok(())
    }
}

fn replicate_missing(src: &mut Sgdb, dst: &mut Sgdb) -> Result<usize, SgdbError> {
    let mut n = 0;
    for layer in [
        MemoryLayer::L2EpisodicShort,
        MemoryLayer::L3EpisodicLong,
        MemoryLayer::L4Semantic,
        MemoryLayer::L5Procedural,
        MemoryLayer::L7Identity,
    ] {
        let prefix = format!("md/{}/", layer.as_str());
        for (sk, _) in src.scan_prefix(&prefix)? {
            if let Ok(Some(rec)) = src.export_record(&sk) {
                match dst.merge_remote(rec)? {
                    MergeVerdict::Applied => n += 1,
                    MergeVerdict::Conflict => {
                        eprintln!("[↔] conflito preservado em {sk}");
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(n)
}

fn telepathy_round(a: &mut Node, b: &mut Node, now: u64) -> Result<(usize, usize), SgdbError> {
    a.crdt.sync(now, &mut a.out)?;
    b.crdt.sync(now, &mut b.out)?;
    a.crdt.sync(now, &mut Pipe::from(b.out.take()))?;
    b.crdt.sync(now, &mut Pipe::from(a.out.take()))?;
    let ab = replicate_missing(&mut a.db, &mut b.db)?;
    let ba = replicate_missing(&mut b.db, &mut a.db)?;
    Ok((ab, ba))
}

fn count_l2_l4(db: &mut Sgdb) -> usize {
    let mut n = 0;
    for layer in [MemoryLayer::L2EpisodicShort, MemoryLayer::L4Semantic] {
        n += db
            .scan_prefix(&format!("md/{}/", layer.as_str()))
            .map(|v| v.len())
            .unwrap_or(0);
    }
    n
}

fn emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    (0..8)
        .map(|_| {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0
        })
        .collect()
}

fn rm_tree(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

fn main() -> Result<(), SgdbError> {
    println!("== Telepatia 2-DB (FileStorage) ==");

    let root = std::env::temp_dir().join(format!("nsgdb-telepathy-2db-{}", std::process::id()));
    rm_tree(&root);
    std::fs::create_dir_all(&root).expect("create temp root for 2-db telepathy");
    let path_a = root.join("agent_a.db");
    let path_b = root.join("agent_b.db");
    println!("[paths] A={} B={}", path_a.display(), path_b.display());

    let mut a = Node::open(1, path_a.clone())?;
    let mut b = Node::open(2, path_b.clone())?;

    a.remember("fact_a", "agente A: preferencia dark mode", &emb(11))?;
    b.remember("fact_b", "agente B: preferencia light mode", &emb(22))?;
    println!(
        "[write] A v={} B v={} (concorrente, DBs isolados)",
        a.crdt.local_version(),
        b.crdt.local_version()
    );

    let (ab1, ba1) = telepathy_round(&mut a, &mut b, 0)?;
    println!("[↔] ronda 1: A→B {ab1} B→A {ba1}");
    let (ab2, ba2) = telepathy_round(&mut a, &mut b, 100)?;
    println!("[↔] ronda 2 (idempotente): A→B {ab2} B→A {ba2}");

    let na = count_l2_l4(&mut a.db);
    let nb = count_l2_l4(&mut b.db);
    println!("[✓] docs vivos A={na} B={nb}");
    assert_eq!(na, 4, "A: fact_a+fact_b (L2+L4 cada)");
    assert_eq!(nb, 4, "B: fact_a+fact_b (L2+L4 cada)");

    let hits = b.db.recall(&emb(11), 3)?;
    assert!(
        hits.iter().any(|h| h.text.contains("dark mode")),
        "B deve recallar memoria de A apos telepatia: {hits:?}"
    );
    println!("[✓] B recall cruzado da memoria de A");

    // ADR-0005: rev key nao pode ser prefixo de fact_a
    a.remember("rev_fact_a", "agente A: dark mode (confirmado)", &emb(33))?;
    a.db.supersede("md/L4/fact_a", "md/L4/rev_fact_a")?;
    a.db.set_validity("md/L4/fact_a", 0, 5000)?;
    a.crdt.record_change();
    let (ab3, ba3) = telepathy_round(&mut a, &mut b, 200)?;
    println!("[↔] ronda 3 (supersede): A→B {ab3} B→A {ba3}");
    assert_eq!(b.db.get_state("md/L4/fact_a")?, MemoryState::Superseded);
    assert!(b.db.validity_at("md/L4/fact_a", 1000)?);
    assert!(!b.db.validity_at("md/L4/fact_a", 6000)?);
    println!("[✓] side-tables (state/validity) no DB de B");

    // Drop handles and reopen from disk — persistence of telepathy result.
    let path_a2 = a.path.clone();
    let path_b2 = b.path.clone();
    drop(a);
    drop(b);
    let mut a2 = Node::reopen(1, path_a2)?;
    let mut b2 = Node::reopen(2, path_b2)?;
    assert_eq!(count_l2_l4(&mut a2.db), count_l2_l4(&mut b2.db));
    assert_eq!(b2.db.get_state("md/L4/fact_a")?, MemoryState::Superseded);
    let hits2 = a2.db.recall_lexical("light mode", 3)?;
    assert!(
        hits2.iter().any(|h| h.text.contains("light mode")),
        "A apos reopen deve ter fato de B no disco"
    );
    println!("[✓] reopen FileStorage: telepatia sobreviveu ao restart");

    rm_tree(&root);
    println!("\nTelepatia 2-DB OK — dois ficheiros, sync CRDT, reopen.");
    Ok(())
}
