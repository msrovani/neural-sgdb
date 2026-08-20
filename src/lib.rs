//! # neural-sgdb
//!
//! Persistent, transferable memory database for AI agents.
//! **Memories, not data.**
//!
//! Core extracted from [neural-os-core](https://github.com/msrovani/neural-os-core)
//! (`k_ai::sgdb`, ADR-0063) as an independent community project, dual-mode
//! `no_std` + `std`, zero external dependencies.
//!
//! - 8 memory layers L0–L7 (Sensory → Working → Episodic → Semantic →
//!   Procedural → Identity)
//! - Semantic `remember` / `recall`: BQ (binary quantization) + FP32 rescore,
//!   SIMD dispatch AVX-512 / AVX2 / scalar
//! - ART (Adaptive Radix Tree) O(k) index for keys/facts
//! - Pluggable storage via `Storage` trait (shipped: `InMemory`, `FileStorage`)
//! - `MemoryDoc` (NMD1) format byte-identical to the parent OS (interop)
//!
//! ## Quick tour (doctest)
//!
//! ```
//! use neural_sgdb::{Sgdb, InMemory, MihIndex, MemoryLayer};
//!
//! let mut db = Sgdb::open(InMemory::new())?;
//!
//! // L1 + L2 (RAM → checkpoint para storage)
//! db.remember_exchange("qual o clima?", "sol, 24 graus")?;
//! db.checkpoint()?;
//!
//! // L4 semântico (BQ + FP32 rescore); você fornece os embeddings
//! let emb = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
//! db.remember_semantic("turno:1", "clima ensolarado em sao paulo", &emb)?;
//!
//! // recall: auto-oversample por dimensionalidade; híbrido lexical opcional
//! let hits = db.recall(&emb, 3)?;
//! let recent = db.recall_weighted(&emb, 3, 1.0, 1.0, 0.5, 1000)?;
//! let lex = db.recall_lexical("ensolarado sao paulo", 3)?;
//! assert_eq!(hits.len(), 1);
//! assert!(!recent.is_empty() && !lex.is_empty());
//!
//! // L3 fato temporal + janela de validade (invalidar-não-deletar)
//! db.remember_fact("user prefere dark mode", 42)?;
//! db.set_validity("md/L3/ts/000000000000002a", 0, 1000)?;
//!
//! // índices diretos (estudo/avançado)
//! let bq = db.bq();                  // acesso ao índice BQ (somente leitura)
//! let mih = MihIndex::build(bq, 4);  // multi-index hashing p/ busca sub-linear
//! # Ok::<(), neural_sgdb::SgdbError>(())
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(feature = "std"), deny(warnings))]

extern crate alloc;

/// Log seam: no_std no-op; std `eprintln!`.
#[macro_export]
macro_rules! sgdb_log {
    ($($arg:tt)*) => {
        #[cfg(feature = "std")]
        {
            eprintln!($($arg)*);
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = core::format_args!($($arg)*);
        }
    };
}

pub mod art;
pub mod arbitration;
pub mod bq;
pub mod conflict;
pub mod embedder;
pub mod era;
pub mod ctype;
pub mod hamming_dispatch;
pub mod lexical;
pub mod limits;
pub mod memory_doc;
pub mod metrics;
pub mod storage;
pub mod tickv;
pub mod trust;

#[cfg(feature = "p2p")]
pub mod crdt;
pub mod lifecycle;

mod engine;
mod sgdb;

#[cfg(test)]
mod wire_fuzz;

pub use art::ArtIndex;
pub use bq::{
    hamming, hamming_path, quantize_f32, quantize_f32_centered, BqFlatIndex, MihIndex,
};
pub use conflict::{generate_conflict_id, ConflictRecord, ConflictStatus};
pub use embedder::{demo_embed, DemoEmbedder, Embedder, DEMO_EMBED_DIM, DEMO_EMBED_NOTE};
pub use era::{estimate_era_migration, EraEstimate, EraReport};
pub use ctype::{detect_content_type, ContentType, RecallPath};
pub use hamming_dispatch::{
    cpu_caps, cpu_has_avx2, cpu_has_avx512, path_name as hamming_kernel_name,
    select_best_hamming_kernel, CpuCaps,
};
pub use lexical::LexicalIndex;
pub use memory_doc::{
    generate_memory_id, LineageEntry, MemoryDoc, MemoryDocView, MemoryLayer, MemoryMeta,
    MemoryRecord, MemoryState, RelationKind, VectorClock,
};
pub use sgdb::{
    HealthReport, Hit, HitProvenance, RememberOptions, RememberOutcome, ScopeDistribution, Sgdb,
    ValidateIssue,
};
pub use storage::{InMemory, Storage, SgdbError};
pub use limits::{
    DEFAULT_SCAN_PAGE_SIZE, MAX_EMBEDDING_DIM, MAX_KLEN, MAX_RAG_CONTEXT_BYTES, MAX_VLEN,
};
pub use tickv::{
    encode_ckpt, encode_record, fnv1a64, scan_volume, CKPT_KEY, MAGIC, record_size,
};
#[cfg(feature = "file-storage")]
pub use storage::FileStorage;
#[cfg(feature = "file-storage")]
pub use tickv::TickvFile;
#[cfg(feature = "p2p")]
pub use crdt::{
    demo as crdt_demo, CrdtMemorySync, MemoryDelta, MemorySnapshot, MemoryVersion, MergePolicy,
    MergeVerdict, SignedEnvelope, Transport, UdpTransport, DEFAULT_P2P_PORT,
};
