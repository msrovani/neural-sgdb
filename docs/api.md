# neural-sgdb — Contrato de API (design target)

> Documento de contrato para a extração do núcleo SGDB do neural-os-core.
> Estado: **design target** — a extração ainda não aconteceu. A API interna
> atual vive em `crates/k_ai/src/sgdb/` do OS mãe; este doc define a superfície
> pública que o crate comunitário deve expor.

## Princípios

1. **Memórias, não dados.** A API fala `remember` / `recall`, camadas L0–L7 e
   transferência de memórias (CRDT) — não `put` / `get` genérico.
2. **Instância, não global.** O OS usa uma static global (`ENGINE`); o crate
   comunitário expõe `Sgdb::open(backend)` — o dev pode abrir quantos bancos
   quiser.
3. **Storage por trait.** Nenhuma dependência de kernel: o dev implementa 4
   métodos e está integrado. Entregamos `InMemory` e `FileStorage` prontos.
4. **Tudo injetável.** Relógio, detecção SIMD e logging são seams, não
   dependências.
5. **`no_std` + `std`.** O mesmo núcleo roda em bare-metal e em host.

## Camadas de memória (L0–L7)

| Camada | Nome | Uso típico |
|--------|------|------------|
| L0 | Sensory | entrada bruta (sensores, rede) |
| L1 | Working | turno atual, contexto imediato |
| L2 | Episódica curta | turnos recentes com timestamp |
| L3 | Episódica longa | fatos e episódios persistentes |
| L4 | Semântica | embeddings BQ + recall vetorial |
| L5 | Procedural | skills / procedimentos |
| L6 | (reservada) | — |
| L7 | Identidade | persona, preferências, estado global |

## Trait `Storage` (o contrato central)

```rust
pub trait Storage {
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError>;
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError>;
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError>;
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError>;
}
```

- **Semântica:** append-log, power-loss safe. `put` é idempotente; `delete`
  grava tombstone. O crate garante CRC + recuperação de crash sobre qualquer
  impl que siga essa semântica.
- **Impls entregues no v0.1:**
  - `InMemory` — RAM, para testes e prototipagem.
  - `FileStorage` — append-log em arquivo, para aplicações std/desktop.
- **Embedded:** o dev implementa sobre o flash dele (SPI/NOR/NVMe). O padrão é o
  do ecossistema embedded-storage: implemente o trait = integrado.

## API pública alvo

```rust
pub struct Sgdb { /* engine + art + bq + storage */ }

impl Sgdb {
    pub fn open(backend: impl Storage) -> Result<Self, SgdbError>;

    // ---- memoria de turno (L1/L2) ----
    pub fn remember_exchange(&mut self, user: &str, response: &str) -> Result<(), SgdbError>;
    pub fn remember_exchange_full(
        &mut self, user: &str, response: &str,
        emb_u: &[f32], emb_a: &[f32], now: u64,
    ) -> Result<(), SgdbError>;

    // ---- semantica (L4, BQ + FP32 rescore) ----
    pub fn remember_semantic(&mut self, key: &str, text: &str, emb: &[f32]) -> Result<(), SgdbError>;

    /// Recall L4: BQ top-k grosso -> rescore FP32 -> top-k fino.
    pub fn recall(&mut self, query: &[f32], k: usize) -> Result<Vec<Hit>, SgdbError>;

    /// RAG: recall + fetch do texto + string formatada pronta pro prompt.
    pub fn rag_context(&mut self, query: &[f32], k: usize) -> Result<String, SgdbError>;

    // ---- fatos (L3, ART por timestamp) ----
    pub fn remember_fact(&mut self, fact: &str, now: u64) -> Result<(), SgdbError>;

    // ---- indice de chaves (ART, O(k)) ----
    pub fn scan_prefix(&mut self, prefix: &str) -> Result<Vec<(String, u64)>, SgdbError>;

    // ---- ciclo de vida ----
    pub fn checkpoint(&mut self) -> Result<(), SgdbError>;
    pub fn prune_working_ram(&mut self) -> Result<usize, SgdbError>;
    pub fn backend(&self) -> &'static str;
    pub fn ready(&self) -> bool;
}

pub struct Hit {
    pub key: String,
    pub text: String,
    pub dist: f32,   // distancia 1-cos (0 = identico)
}
```

## Seams injetáveis

| Seam | Hoje (kernel) | Vira |
|------|---------------|------|
| Clock | `k_nano::interrupts::TIMER_TICKS` | parâmetro `now: u64` nos métodos com timestamp |
| CPU/SIMD | `k_nano::platform_probe::hw_info()` | `std::arch::is_x86_feature_detected!` no host; `cpu_caps()` injetável em no_std |
| Logging | `k_nano::slog_kai!` (serial) | macro interna + hook `log` opcional |
| Storage | `k_nano::storage` (NVMe/RAM) | `trait Storage` (acima) |

## Formato on-disk (interop com o OS)

- **Records:** `TKLV` (klen/vlen/crc32, tombstone V=0) e `TKCK` (checkpoint).
- **MemoryDoc:** `NMD1` (layer, chave, VectorClock 8 nós, payload, bitvec).
- **Contrato:** um volume gravado pelo neural-os-core **é lido** pelo
  neural-sgdb e vice-versa. Formatos autodescritos no código-fonte; esta
  compatibilidade é um requisito de aceite da extração.

## O que NÃO vai para o público

- **Namespaces OS:** `hanr/`, `pkg/`, `audit/`, `sys/`, `hw/` são específicos
  do AIOS e ficam internos no kernel. O crate comunitário expõe apenas o modelo
  de memória (`md/L0`–`md/L7`).
- **CRDT sync (rede):** vira feature opcional `p2p` — degrada para local-only
  quando desligada.
- **Residuals OS:** benchmark 10M/100k, kill-9 HW, AVX-512 CI — são metas do OS
  mãe; o crate publica seus próprios benchmarks quando existirem.

## Exemplo (vitrine README)

```rust
use neural_sgdb::{Sgdb, FileStorage};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Sgdb::open(FileStorage::open("agent_memory.db")?)?;

    db.remember_exchange("qual o clima?", "sol, 24 graus")?;
    db.remember_semantic("turno:1", "clima ensolarado em sao paulo", &emb)?;

    let hits = db.recall(&query_emb, 5)?;
    let ctx = db.rag_context(&query_emb, 3)?;
    println!("{ctx}");
    Ok(())
}
```

## Mapa de migração (interna → pública)

| Interna (`k_ai::sgdb`) | Pública (`neural_sgdb`) | Mudança |
|------------------------|--------------------------|---------|
| `init_global(1)` / `ensure_ready()` | `Sgdb::open(backend)` | static global → instância |
| `remember_exchange(u, r)` | `Sgdb::remember_exchange(u, r)` | wrapper |
| `remember_semantic(k, t, emb)` | `Sgdb::remember_semantic(k, t, emb)` | wrapper |
| `recall_semantic(q, k) -> (Vec<(String,u32)>, &'static str)` | `recall(q, k) -> Vec<Hit>` | tipo de retorno |
| `rag_context(q, k) -> String` | `rag_context(q, k) -> Result<String>` | erro |
| `remember_fact(f)` (usa TIMER_TICKS) | `remember_fact(f, now)` | clock injetado |
| `put_kv` / `get_kv` | via `Storage` trait | backend |
| `slog_kai!` | macro `log` interna | seam |
| `store::ns::{hanr,pkg,...}` | removidos | OS-only |

## Critérios de aceite da extração

- [ ] `cargo test` no repo neural-sgdb passa (host) com `InMemory` + `FileStorage`
- [ ] `cargo check --no-default-features --target x86_64-unknown-none` passa (no_std, alloc-only, zero deps)
- [ ] Roundtrip `FileStorage`: put → reopen → get, sobrevive a crash simulado
- [ ] **Interop de documento (v0.1):** `MemoryDoc` (NMD1) — encode/decode
      byte-idêntico ao do OS (`crates/k_ai/src/sgdb/memory_doc.rs`); um documento
      NMD1 escrito por um é lido pelo outro
- [ ] **Interop de storage (pós-v0.1):** `FileStorage` replicar o formato de
      registros `TKLV`/`TKCK` do TickvLite para compatibilidade byte a byte de
      volumes — adiado (FileStorage v0.1 usa append-log próprio, CRC por registro)
- [ ] Zero dependência de `k_nano` / kernel no código do crate

## Nota — relação com o OS (Modo 1)

Repo separado, evolução independente. O neural-os-core **mantém** `k_ai::sgdb`
interno (AGPL) — não há fiação (path dep nem versão) neste momento. O ponto de
compatibilidade entre os dois é o **formato de documento NMD1** (acima). Se um
dia o OS passar a consumir o produto do repo, será por versão publicada no
crates.io, não por acoplamento de filesystem.
