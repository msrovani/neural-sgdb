//! **Telepatia do neural-sgdb** — troca de memórias entre duas instâncias via
//! p2p (CRDT de versões + pull/diff de docs).
//!
//! Run: `cargo run --release --example p2p_telepathy --features p2p`
//!
//! Como funciona:
//! 1. Cada instância tem um `Sgdb` + um `CrdtMemorySync` (nó A e nó B).
//! 2. `remember_*` escreve memória local e `record_change()` bumpa a versão.
//! 3. `sync()` troca as versões por um transporte (aqui: fila em memória —
//!    no mundo real: `UdpTransport`, TLS, serial).
//! 4. Quando um nó aprende que o peer avançou, ele puxa os docs que ainda não
//!    tem (`Sgdb::get` → `Sgdb::put` — diff idempotente por storage key).
//!
//! As duas instâncias convergem sem um "servidor central" — cada uma fala a
//! sua versão e replica o que falta. É a telepatia: memória, não pacotes.

use neural_sgdb::{
    CrdtMemorySync, InMemory, MemoryLayer, MemoryState, MergeVerdict, Sgdb, SgdbError, Transport,
};

/// Fila em memória entre dois nós (loopback). No mundo real: UDP/TLS/serial.
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

/// Uma instância "viva": banco + relógio CRDT + fila de saída.
struct Node {
    db: Sgdb,
    crdt: CrdtMemorySync,
    out: Pipe,
}

impl Node {
    fn new(id: u8) -> Self {
        Node {
            db: Sgdb::open(InMemory::new()).expect("open"),
            crdt: CrdtMemorySync::new(id),
            out: Pipe::default(),
        }
    }

    fn remember(&mut self, key: &str, text: &str, emb: &[f32]) -> Result<(), SgdbError> {
        self.db.remember_semantic(key, text, emb)?;
        self.crdt.record_change();
        Ok(())
    }
}

/// Replica de `src` para `dst` via `MemoryRecord` (doc + estado + validade) e
/// `merge_remote` (política por camada): aplica o que falta, reaplica
/// side-metadata avançado, preserva conflitos. Idempotente — o CRDT de
/// versões só dispara quando convém.
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
        let keys = src.scan_prefix(&prefix)?;
        for (sk, _) in keys {
            if let Ok(Some(rec)) = src.export_record(&sk) {
                match dst.merge_remote(rec)? {
                    MergeVerdict::Applied => n += 1,
                    MergeVerdict::Conflict => {
                        eprintln!("[↔] conflito preservado em {sk} (camada superior resolve)");
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(n)
}

/// Ronda de telepatia: A e B publicam versões, recebem as do outro e replicam
/// o que falta. Retorna (docs replicados A→B, B→A).
fn telepathy_round(a: &mut Node, b: &mut Node, now: u64) -> Result<(usize, usize), SgdbError> {
    // TX: cada um publica para a fila do outro
    a.crdt.sync(now, &mut a.out)?;
    b.crdt.sync(now, &mut b.out)?;
    // RX: cada um drena a fila que recebeu
    a.crdt.sync(now, &mut Pipe::from(b.out.take()))?;
    b.crdt.sync(now, &mut Pipe::from(a.out.take()))?;
    // pull/diff dos docs que faltam
    let ab = replicate_missing(&mut a.db, &mut b.db)?;
    let ba = replicate_missing(&mut b.db, &mut a.db)?;
    Ok((ab, ba))
}

fn main() -> Result<(), SgdbError> {
    println!("== Telepatia neural-sgdb: 2 instâncias, troca p2p ==");

    let mut a = Node::new(1);
    let mut b = Node::new(2);

    // embeddings de demonstração (determinísticos; troque por reais)
    let emb = |seed: u64| -> Vec<f32> {
        let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (0..8)
            .map(|_| {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0
            })
            .collect()
    };

    // A lembra uma memória; B lembra outra — sem se conhecerem ainda
    a.remember("m1", "eu sou a instancia A, memória um", &emb(1))?;
    println!("[A] lembra m1  (versão A = {})", a.crdt.local_version());
    b.remember("m2", "eu sou a instancia B, memória dois", &emb(2))?;
    println!("[B] lembra m2  (versão B = {})", b.crdt.local_version());

    // ronda 1: elas se "veem" e trocam versões
    let (ab1, ba1) = telepathy_round(&mut a, &mut b, 0)?;
    println!("[↔] ronda 1: A→B {ab1} doc(s), B→A {ba1} doc(s)");

    // ronda 2: convergência total (replicação é idempotente)
    let (ab2, ba2) = telepathy_round(&mut a, &mut b, 200)?;
    println!("[↔] ronda 2: A→B {ab2} doc(s), B→A {ba2} doc(s)");

    // B responde à memória de A (escrita local nova) → telepatia de volta
    b.remember("m3", "resposta de B para A", &emb(3))?;
    println!("[B] lembra m3  (versão B = {})", b.crdt.local_version());
    let (ab3, ba3) = telepathy_round(&mut a, &mut b, 400)?;
    println!("[↔] ronda 3: A→B {ab3} doc(s), B→A {ba3} doc(s)");

    // verificação: as DUAS instâncias conhecem as TRÊS memórias (telepatia)
    let count = |db: &mut Sgdb| -> usize {
        let mut n = 0;
        for layer in [MemoryLayer::L2EpisodicShort, MemoryLayer::L4Semantic] {
            n += db.scan_prefix(&format!("md/{}/", layer.as_str())).map(|v| v.len()).unwrap_or(0);
        }
        n
    };
    let na = count(&mut a.db);
    let nb = count(&mut b.db);
    println!("[✓] A conhece {na} docs, B conhece {nb} docs");
    assert_eq!(na, 6, "A deveria ter as 3 memórias (L2 text + L4 emb)");
    assert_eq!(nb, 6, "B deveria ter as 3 memórias (L2 text + L4 emb)");

    // recall cruzado: B recupera a memória escrita por A (semântica)
    let hits_b = b.db.recall(&emb(1), 3)?;
    let texts_b: Vec<&str> = hits_b.iter().map(|h| h.text.as_str()).collect();
    println!("[B] recall da memória de A: {texts_b:?}");
    assert!(
        texts_b.iter().any(|t| t.contains("instancia A")),
        "B deveria recuperar a memória de A via recall"
    );

    // ── side-metadata viaja (P0-5): A supersede m1 e marca validade ──────
    a.remember("m1b", "eu sou a instancia A, memoria um (revisada)", &emb(5))?;
    a.db.supersede("md/L4/m1", "md/L4/m1b")?;
    a.db.set_validity("md/L4/m1", 0, 2000)?;
    a.crdt.record_change();
    let (ab4, ba4) = telepathy_round(&mut a, &mut b, 600)?;
    println!("[↔] ronda 4 (supersede + validade): A→B {ab4} doc(s), B→A {ba4} doc(s)");

    // B vê o estado e a validade replicados (o record carrega as side-tables)
    assert_eq!(b.db.get_state("md/L4/m1").unwrap(), MemoryState::Superseded);
    assert!(b.db.validity_at("md/L4/m1", 1000).unwrap());
    assert!(!b.db.validity_at("md/L4/m1", 2500).unwrap());
    // lineage viaja: m1b tem m1 como pai (DAG causal)
    let parents = b.db.meta("md/L4/m1b").unwrap().unwrap().parent_ids;
    assert!(parents.contains(&a.db.memory_id("md/L4/m1").unwrap().unwrap()));
    println!("[✓] estado/validade/lineage replicados em B (contradição #2 fechada)");

    println!("\nTelepatia OK — as duas instâncias convergiram via p2p.");
    Ok(())
}
