//! # neural-sgdb
//!
//! Banco de memória persistente e transferível para agentes de IA.
//! **Memórias, não dados.**
//!
//! Núcleo extraído do [neural-os-core](https://github.com/msrovani/neural-os-core)
//! (`k_ai::sgdb`, ADR-0063) como projeto comunitário independente, dual-mode
//! `no_std` + `std`, zero dependências externas.
//!
//! - 8 camadas de memória L0–L7 (Sensory → Working → Episódica → Semântica →
//!   Procedural → Identidade)
//! - `remember` / `recall` semântico: BQ (quantização binária) + rescore FP32,
//!   dispatch SIMD AVX-512 / AVX2 / scalar
//! - Índice ART (Adaptive Radix Tree) O(k) para chaves/fatos
//! - Storage plugável via `Storage` trait (entregues: `InMemory`, `FileStorage`)
//! - Formato `MemoryDoc` (NMD1) byte-idêntico ao do OS mãe (interop)
//!
//! ## Exemplo
//!
//! ```
//! use neural_sgdb::{Sgdb, InMemory};
//!
//! let mut db = Sgdb::open(InMemory::new())?;
//! db.remember_exchange("qual o clima?", "sol, 24 graus")?;
//! let facts = db.scan_prefix("md/L1/")?;
//! assert!(facts.len() >= 1);
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
pub mod bq;
pub mod hamming_dispatch;
pub mod memory_doc;
pub mod storage;
pub mod tickv;

#[cfg(feature = "p2p")]
pub mod crdt;

mod engine;
mod sgdb;

pub use art::ArtIndex;
pub use bq::{hamming, hamming_path, quantize_f32, BqFlatIndex};
pub use hamming_dispatch::{
    cpu_caps, cpu_has_avx2, cpu_has_avx512, path_name as hamming_kernel_name,
    select_best_hamming_kernel, CpuCaps,
};
pub use memory_doc::{MemoryDoc, MemoryDocView, MemoryLayer, VectorClock};
pub use sgdb::{Hit, Sgdb};
pub use storage::{InMemory, Storage, SgdbError};
pub use tickv::{
    encode_ckpt, encode_record, fnv1a64, scan_volume, CKPT_KEY, MAGIC, MAX_KLEN, MAX_VLEN,
    record_size,
};
#[cfg(feature = "std")]
pub use storage::FileStorage;
#[cfg(feature = "std")]
pub use tickv::TickvFile;
#[cfg(feature = "p2p")]
pub use crdt::{demo as crdt_demo, CrdtMemorySync, Transport, UdpTransport, DEFAULT_P2P_PORT};
