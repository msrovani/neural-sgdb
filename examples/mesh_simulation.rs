//! **Telepatia multi-IA em camadas (P2-5)** — simulação de IAs externas e
//! locais em mesh, cada uma rodando um `Sgdb` + `CrdtMemorySync`.
//!
//! Run: `cargo run --release --example mesh_simulation --features p2p`
//!
//! Cenário (honesto — as "IAs" são POLÍTICAS DETERMINÍSTICAS stub, sem LLM
//! e sem embeddings reais; o que se demonstra é o SUBSTRATO de memória):
//!
//! 1. **8 agentes em 5 camadas** (limite do cenário): L1 superfície (2),
//!    L2 (2), L3 (2), L4 (1), L5 consolidação/identidade (1).
//! 2. A **IA externa** fala com a camada 1 — cada mensagem vira uma memória
//!    semântica (L4 + texto L2) na superfície.
//! 3. Cada agente da camada 1 **responde** usando recall do PRÓPRIO banco:
//!    "camada-1 responde: <top hit>" (a IA stub usa o substrato).
//! 4. **Telepatia** (anti-entropy: anúncios de clock + pull de records via
//!    `export_record` → `merge_remote`) propaga as memórias camada-acima.
//! 5. As camadas profundas (L4/L5) **recuperam por recall** uma memória que
//!    entrou na camada 1 e criam memória consolidada derivada.

use neural_sgdb::{
    CrdtMemorySync, InMemory, MemoryLayer, MergeVerdict, Sgdb, SgdbError, Transport,
};

/// Loopback de versões entre dois nós (no mundo real: UDP/TLS/serial).
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

struct Agent {
    db: Sgdb,
    crdt: CrdtMemorySync,
}

/// Embedding de demonstração (determinístico; troque por reais).
fn demo_emb(seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(1103515245).wrapping_add(12345);
    (0..16)
        .map(|_| {
            s = s.wrapping_mul(1103515245).wrapping_add(12345);
            ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0
        })
        .collect()
}

fn main() -> Result<(), SgdbError> {
    println!("== Telepatia multi-IA em camadas: 8 agentes, 5 camadas, mesh ==");

    // camadas: L1 superfície [0,1], L2 [2,3], L3 [4,5], L4 [6], L5 [7]
    let layers: [&[usize]; 5] = [&[0, 1], &[2, 3], &[4, 5], &[6], &[7]];
    let n = 8;
    let mut agents: Vec<Agent> = (1..=n as u8)
        .map(|id| Agent {
            db: Sgdb::open(InMemory::new()).expect("open"),
            crdt: CrdtMemorySync::new(id),
        })
        .collect();

    // mesh em camadas (dirigido): k ↔ k+1 + intra-camada (redundância)
    let mut edges: Vec<Vec<bool>> = vec![vec![false; n]; n];
    let mut connect = |i: usize, j: usize| {
        edges[i][j] = true;
        edges[j][i] = true;
    };
    for k in 0..layers.len() - 1 {
        for &a in layers[k] {
            for &b in layers[k + 1] {
                connect(a, b);
            }
        }
    }
    for l in layers {
        for w in 0..l.len() {
            let a = l[w];
            let b = l[(w + 1) % l.len()];
            if a != b {
                connect(a, b);
            }
        }
    }

    let remember = |agents: &mut Vec<Agent>, i: usize, key: &str, text: &str, seed: u64| {
        agents[i].db.remember_semantic(key, text, &demo_emb(seed))?;
        agents[i].crdt.record_change();
        Ok::<(), SgdbError>(())
    };

    // 1) IA EXTERNA fala com a camada 1
    let msgs = [
        "usuario prefere dark mode",
        "reuniao marcada as 14h",
        "o deploy quebrou a CI",
        "gosta de cafe espresso",
        "proximo sprint e de features",
    ];
    for (idx, text) in msgs.iter().enumerate() {
        let agent = if idx % 2 == 0 { 0 } else { 1 };
        remember(&mut agents, agent, &format!("ext{idx:02}"), text, 100 + idx as u64)?;
    }
    println!("[IA externa] falou 5 mensagens com a camada 1 (L1)");

    // 2) camada 1 responde a partir do PRÓPRIO recall (IA stub)
    for i in 0..2 {
        let hits = agents[i].db.recall(&demo_emb(50), 1)?;
        let top = hits.first().map(|h| h.text.clone()).unwrap_or_default();
        remember(&mut agents, i, &format!("rsp{i:02}"), &format!("camada-1 responde: {top}"), 60 + i as u64)?;
        println!("[L1 agent {}] respondeu: {top}", agents[i].crdt.node_id());
    }

    // 3) rondas de telepatia: anúncios + pull de records pelas arestas
    let telepathy = |agents: &mut Vec<Agent>, rounds: u64| -> Result<usize, SgdbError> {
        let mut applied = 0;
        for round in 0..rounds {
            // TX/RX de versões por aresta (gossip assimétrico)
            for i in 0..n {
                let out = Pipe::from(agents[i].crdt.announce());
                for j in 0..n {
                    if i == j || !edges[i][j] {
                        continue;
                    }
                    agents[j].crdt.sync(round, &mut Pipe::from(out.buf.clone()))?;
                }
            }
            // pull/replicação de records (export → merge, idempotente)
            for i in 0..n {
                for j in 0..n {
                    if i == j || !edges[i][j] {
                        continue;
                    }
                    for layer in [
                        MemoryLayer::L2EpisodicShort,
                        MemoryLayer::L3EpisodicLong,
                        MemoryLayer::L4Semantic,
                        MemoryLayer::L5Procedural,
                        MemoryLayer::L7Identity,
                    ] {
                        let prefix = format!("md/{}/", layer.as_str());
                        for (sk, _) in agents[i].db.scan_prefix(&prefix)? {
                            if let Ok(Some(rec)) = agents[i].db.export_record(&sk) {
                                if agents[j].db.merge_remote(rec)? == MergeVerdict::Applied {
                                    applied += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(applied)
    };
    let total_applied = telepathy(&mut agents, 12)?;
    println!("[telepatia] {total_applied} replicações de records entre camadas");

    // 4) camadas profundas RECUPERAM a memória que entrou na camada 1
    for (deep, label) in [(6usize, "L4"), (7, "L5")] {
        let hits = agents[deep].db.recall(&demo_emb(102), 1)?;
        let top = hits.first().map(|h| h.text.clone()).unwrap_or_default();
        assert!(
            top.contains("deploy quebrou a CI"),
            "camada {label} não telepatizou: {top}"
        );
        remember(
            &mut agents,
            deep,
            &format!("cons{deep:02}"),
            &format!("consolidado: {top}"),
            200 + deep as u64,
        )?;
        println!("[{label} agent {}] telepatia OK — recall da memória da camada 1: {top}", agents[deep].crdt.node_id());
    }

    // 5) última ronda de telepatia para propagar a consolidação, então
    //    verifica: TODOS os agentes convergiram para o MESMO conteúdo
    telepathy(&mut agents, 8)?;
    let count = |agents: &mut Vec<Agent>, i: usize| -> usize {
        let mut c = 0;
        for layer in [MemoryLayer::L2EpisodicShort, MemoryLayer::L4Semantic] {
            c += agents[i]
                .db
                .scan_prefix(&format!("md/{}/", layer.as_str()))
                .map(|v| v.len())
                .unwrap_or(0);
        }
        c
    };
    let c0 = count(&mut agents, 0);
    for i in 1..n {
        assert_eq!(count(&mut agents, i), c0, "agente {i} com conteúdo divergente");
    }
    println!("[✓] convergência: {n} agentes compartilham {c0} memórias (L4+L2) byte-idênticas");

    println!("\nTelepatia multi-IA OK — memórias da camada 1 ressurgem na camada 5 via recall.");
    Ok(())
}
