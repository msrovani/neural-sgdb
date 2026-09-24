//! IDX1 — snapshot do índice derivado para fast-mount (v1.1.29, ADR-0009 §3).
//!
//! O `Sgdb::open` rebuilda TODOS os índices derivados do storage a cada
//! mount (custo ~linear em docs vivos; medido no ADR-0009 §4). Este wire
//! type persiste o estado derivado NECESSÁRIO ao fast-mount; o storage
//! (NMD1 + side-tables) continua sendo a FONTE DA VERDADE — snapshot
//! corrompido/stale/ausente → full rebuild (nunca autoridade sobre o
//! storage, mesmo contrato do TKCK em `tickv.rs`, ADR-0004).
//!
//! O que entra no snapshot (e por quê):
//! - **ART key set** — as storage keys indexadas. IDs do ART ficam FORA:
//!   `NEXT_ID` é contador global de processo (mesma regra do
//!   `index_fingerprint`, ADR-0011); o fast-mount re-assina ids novos.
//! - **`indexed_dims`** — a fonte da verdade da era (S1) e do guard de write.
//! - **`entity_index`** — entidade → storage keys (derivado de `sys/meta/`,
//!   mas custa um scan completo de metas para reconstruir).
//! - **`corpus_counts`** — contagem de vetores por dim (igualdade exata no
//!   validate); as SOMAS f64 ficam FORA (não-associativas — o fast-mount
//!   recalcula as somas, ou aceita o custo de um rescore sem média até o
//!   primeiro reinforce; a decisão de política é do fast-mount, não do codec).
//! - **`words_per_vec`** do BQ + contagem de vetores vivos — geometria de era.
//! - **`fingerprint`** — o valor do `index_fingerprint` no momento do
//!   snapshot: o fast-mount valida o mount contra o storage com este oráculo
//!   (ADR-0011) antes de confiar nele.
//!
//! O lexical index NÃO entra: BM25 sobre textos L2/L3 precisa dos DFs por
//! termo e o ganho de pular o rebuild lexical é menor (só L2/L3 são
//! tokenizados); a política de incluí-lo é de um commit futuro se o bench
//! medir que o scan de textos domina. Custo do lexical no open fica honesto
//! no `open_rebuild_ms`.
//!
//! `no_std`-safe (só `alloc`), zero deps, decode bounds-checked (nunca panics).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

pub const IDX_MAGIC: &[u8; 4] = b"IDX1";
pub const IDX_VERSION: u8 = 1;

/// Snapshot do índice derivado (um "TKCK para os índices").
#[derive(Clone, Debug, PartialEq)]
pub struct IndexSnapshot {
    /// Storage keys indexadas no ART (sem ids — ver doc do módulo).
    pub art_keys: Vec<String>,
    /// Entidade → storage keys (listas já ordenadas pelo coletor).
    pub entity_index: Vec<(String, Vec<String>)>,
    /// Dimensões indexadas no BQ (era).
    pub indexed_dims: Vec<usize>,
    /// (dim, count de vetores vivos) — igualdade exata no validate.
    pub corpus_counts: Vec<(usize, u64)>,
    /// Largura do BQ em words (64 bits) por vetor (era invariant).
    pub bq_words_per_vec: u32,
    /// Vetores vivos no BQ (não inclui órfãos — inerte por design).
    pub bq_live: u64,
    /// `index_fingerprint` do índice que gerou o snapshot (oráculo do mount).
    pub fingerprint: u64,
    /// Tick do caller na escrita do snapshot (staleness de política do host).
    pub written_at: u64,
    /// Postings do índice lexical: termo → (doc, tf) — listas em ordem
    /// canônica (BTreeMap do coletor). Sem isto o fast-mount não reproduz o
    /// fingerprint (o fp cobre o lexical, ADR-0011).
    pub lexical_postings: Vec<(String, Vec<(String, u32)>)>,
    /// doc_len do lexical: doc → nº de tokens.
    pub lexical_doc_len: Vec<(String, u32)>,
    /// n_docs do lexical (contrato do BM25 idf).
    pub lexical_n_docs: u32,
    /// Vetores vivos do BQ: (storage key, words do bitvec). Os ids do flat
    /// são re-assinados no mount (contador de processo) — o fp resolve via
    /// id_to_sk, então a pareamento sk→words é o que preserva o fingerprint.
    pub bq_entries: Vec<(String, Vec<u64>)>,
}

/// Entrada derivada: uma key do ART + os vetores vivos associados (o
/// fast-mount precisa parear key → bitvec sem depender de ids persistidos).
#[derive(Clone, Debug, PartialEq)]
pub struct IdxKeyVecs {
    pub sk: String,
    /// Words do bitvec (vazio p/ docs sem BQ: L1/L3/rel/…).
    pub words: Vec<u64>,
}

impl IndexSnapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(48 + self.art_keys.len() * 16 + self.entity_index.len() * 24);
        out.extend_from_slice(IDX_MAGIC);
        out.push(IDX_VERSION);
        out.extend_from_slice(&self.fingerprint.to_le_bytes());
        out.extend_from_slice(&self.written_at.to_le_bytes());
        out.extend_from_slice(&self.bq_words_per_vec.to_le_bytes());
        out.extend_from_slice(&self.bq_live.to_le_bytes());
        // indexed_dims
        out.extend_from_slice(&(self.indexed_dims.len() as u32).to_le_bytes());
        for &d in &self.indexed_dims {
            out.extend_from_slice(&(d as u32).to_le_bytes());
        }
        // corpus_counts
        out.extend_from_slice(&(self.corpus_counts.len() as u32).to_le_bytes());
        for &(d, c) in &self.corpus_counts {
            out.extend_from_slice(&(d as u32).to_le_bytes());
            out.extend_from_slice(&c.to_le_bytes());
        }
        // art keys
        out.extend_from_slice(&(self.art_keys.len() as u32).to_le_bytes());
        for k in &self.art_keys {
            out.extend_from_slice(&(k.len() as u16).to_le_bytes());
            out.extend_from_slice(k.as_bytes());
        }
        // entity index
        out.extend_from_slice(&(self.entity_index.len() as u32).to_le_bytes());
        for (ent, skeys) in &self.entity_index {
            out.extend_from_slice(&(ent.len() as u16).to_le_bytes());
            out.extend_from_slice(ent.as_bytes());
            out.extend_from_slice(&(skeys.len() as u32).to_le_bytes());
            for sk in skeys {
                out.extend_from_slice(&(sk.len() as u16).to_le_bytes());
                out.extend_from_slice(sk.as_bytes());
            }
        }
        // lexical postings: termo → [(doc, tf)]
        out.extend_from_slice(&(self.lexical_postings.len() as u32).to_le_bytes());
        for (term, docs) in &self.lexical_postings {
            out.extend_from_slice(&(term.len() as u16).to_le_bytes());
            out.extend_from_slice(term.as_bytes());
            out.extend_from_slice(&(docs.len() as u32).to_le_bytes());
            for (doc, tf) in docs {
                out.extend_from_slice(&(doc.len() as u16).to_le_bytes());
                out.extend_from_slice(doc.as_bytes());
                out.push((*tf).min(u8::MAX as u32) as u8);
                // tf acima de 255 não existe no tokenize por doc realista;
                // min() é cinto E suspensório: tf satura em vez de truncar o stream.
                if *tf > u8::MAX as u32 {
                    out.extend_from_slice(&tf.to_le_bytes());
                }
            }
        }
        // lexical doc_len
        out.extend_from_slice(&(self.lexical_doc_len.len() as u32).to_le_bytes());
        for (doc, l) in &self.lexical_doc_len {
            out.extend_from_slice(&(doc.len() as u16).to_le_bytes());
            out.extend_from_slice(doc.as_bytes());
            out.extend_from_slice(&l.to_le_bytes());
        }
        out.extend_from_slice(&self.lexical_n_docs.to_le_bytes());
        // BQ entries: (sk, words u64 le)
        out.extend_from_slice(&(self.bq_entries.len() as u32).to_le_bytes());
        for (sk, words) in &self.bq_entries {
            out.extend_from_slice(&(sk.len() as u16).to_le_bytes());
            out.extend_from_slice(sk.as_bytes());
            out.extend_from_slice(&(words.len() as u16).to_le_bytes());
            for w in words {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    /// Decode bounds-checked (nunca panics em entrada malformada/truncada).
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 25 || &data[0..4] != IDX_MAGIC {
            return Err("bad idx magic");
        }
        if data[4] != IDX_VERSION {
            return Err("bad idx version");
        }
        let mut off = 5;
        let fingerprint = rd_u64(data, off).ok_or("trunc fp")?;
        off += 8;
        let written_at = rd_u64(data, off).ok_or("trunc ts")?;
        off += 8;
        let bq_words_per_vec = rd_u32(data, off).ok_or("trunc wpv")?;
        off += 4;
        let bq_live = rd_u64(data, off).ok_or("trunc live")?;
        off += 8;
        let ndims = rd_u32(data, off).ok_or("trunc ndims")? as usize;
        off += 4;
        // bounds: dims <= MAX_EMBEDDING_DIM + folga; nunca alocar do wire.
        if ndims > crate::limits::MAX_EMBEDDING_DIM {
            return Err("ndims over limit");
        }
        let mut indexed_dims = Vec::with_capacity(ndims);
        for _ in 0..ndims {
            let d = rd_u32(data, off).ok_or("trunc dim")? as usize;
            off += 4;
            indexed_dims.push(d);
        }
        let ncounts = rd_u32(data, off).ok_or("trunc ncounts")? as usize;
        off += 4;
        if ncounts > crate::limits::MAX_EMBEDDING_DIM {
            return Err("ncounts over limit");
        }
        let mut corpus_counts = Vec::with_capacity(ncounts);
        for _ in 0..ncounts {
            let d = rd_u32(data, off).ok_or("trunc cdim")? as usize;
            off += 4;
            let c = rd_u64(data, off).ok_or("trunc ccount")?;
            off += 8;
            corpus_counts.push((d, c));
        }
        let nkeys = rd_u32(data, off).ok_or("trunc nkeys")? as usize;
        off += 4;
        if nkeys > crate::limits::MAX_KLEN * 1024 {
            return Err("nkeys over limit");
        }
        let mut art_keys = Vec::with_capacity(nkeys.min(1 << 20));
        for _ in 0..nkeys {
            let kl = rd_u16(data, off).ok_or("trunc klen")? as usize;
            off += 2;
            if kl > crate::limits::MAX_KLEN {
                return Err("klen over limit");
            }
            let kb = data
                .get(off..off.checked_add(kl).ok_or("klen overflow")?)
                .ok_or("trunc key")?;
            off += kl;
            art_keys.push(core::str::from_utf8(kb).map_err(|_| "utf8 key")?.into());
        }
        let nents = rd_u32(data, off).ok_or("trunc nents")? as usize;
        off += 4;
        let mut entity_index = Vec::with_capacity(nents.min(1 << 16));
        for _ in 0..nents {
            let el = rd_u16(data, off).ok_or("trunc elen")? as usize;
            off += 2;
            let eb = data
                .get(off..off.checked_add(el).ok_or("elen overflow")?)
                .ok_or("trunc ent")?;
            off += el;
            let ent: String = core::str::from_utf8(eb).map_err(|_| "utf8 ent")?.into();
            let nsk = rd_u32(data, off).ok_or("trunc nsk")? as usize;
            off += 4;
            let mut skeys = Vec::with_capacity(nsk.min(1 << 16));
            for _ in 0..nsk {
                let sl = rd_u16(data, off).ok_or("trunc slen")? as usize;
                off += 2;
                if sl > crate::limits::MAX_KLEN {
                    return Err("slen over limit");
                }
                let sb = data
                    .get(off..off.checked_add(sl).ok_or("slen overflow")?)
                    .ok_or("trunc skey")?;
                off += sl;
                skeys.push(core::str::from_utf8(sb).map_err(|_| "utf8 skey")?.into());
            }
            entity_index.push((ent, skeys));
        }
        // lexical postings
        let nterms = rd_u32(data, off).ok_or("trunc nterms")? as usize;
        off += 4;
        let mut lexical_postings = Vec::with_capacity(nterms.min(1 << 16));
        for _ in 0..nterms {
            let tl = rd_u16(data, off).ok_or("trunc tlen")? as usize;
            off += 2;
            let tb = data
                .get(off..off.checked_add(tl).ok_or("tlen overflow")?)
                .ok_or("trunc term")?;
            off += tl;
            let term: String = core::str::from_utf8(tb).map_err(|_| "utf8 term")?.into();
            let ndocs = rd_u32(data, off).ok_or("trunc ndocs")? as usize;
            off += 4;
            let mut docs = Vec::with_capacity(ndocs.min(1 << 16));
            for _ in 0..ndocs {
                let dl = rd_u16(data, off).ok_or("trunc dlen")? as usize;
                off += 2;
                let db = data
                    .get(off..off.checked_add(dl).ok_or("dlen overflow")?)
                    .ok_or("trunc ldoc")?;
                off += dl;
                let doc: String = core::str::from_utf8(db).map_err(|_| "utf8 ldoc")?.into();
                let tf_b = *data.get(off).ok_or("trunc tf")?;
                off += 1;
                let tf = if tf_b == u8::MAX {
                    // possível tf saturado: lê o u32 completo
                    let full = rd_u32(data, off).ok_or("trunc tf_full")?;
                    off += 4;
                    full
                } else {
                    tf_b as u32
                };
                docs.push((doc, tf));
            }
            lexical_postings.push((term, docs));
        }
        // lexical doc_len
        let ndl = rd_u32(data, off).ok_or("trunc ndl")? as usize;
        off += 4;
        let mut lexical_doc_len = Vec::with_capacity(ndl.min(1 << 16));
        for _ in 0..ndl {
            let dl = rd_u16(data, off).ok_or("trunc dllen")? as usize;
            off += 2;
            let db = data
                .get(off..off.checked_add(dl).ok_or("dllen overflow")?)
                .ok_or("trunc ldoc_len")?;
            off += dl;
            let doc: String = core::str::from_utf8(db).map_err(|_| "utf8 ldoc_len")?.into();
            let l = rd_u32(data, off).ok_or("trunc doclen")?;
            off += 4;
            lexical_doc_len.push((doc, l));
        }
        let lexical_n_docs = rd_u32(data, off).ok_or("trunc ndocs_lex")?;
        off += 4;
        // BQ entries
        let nbq = rd_u32(data, off).ok_or("trunc nbq")? as usize;
        off += 4;
        let mut bq_entries = Vec::with_capacity(nbq.min(1 << 16));
        for _ in 0..nbq {
            let sl = rd_u16(data, off).ok_or("trunc bq_slen")? as usize;
            off += 2;
            if sl > crate::limits::MAX_KLEN {
                return Err("bq slen over limit");
            }
            let sb = data
                .get(off..off.checked_add(sl).ok_or("bq slen overflow")?)
                .ok_or("trunc bq_sk")?;
            off += sl;
            let sk: String = core::str::from_utf8(sb).map_err(|_| "utf8 bq_sk")?.into();
            let nw = rd_u16(data, off).ok_or("trunc bq_nw")? as usize;
            off += 2;
            if nw > crate::limits::MAX_EMBEDDING_DIM.div_ceil(64) {
                return Err("bq nw over limit");
            }
            let mut words = Vec::with_capacity(nw);
            for _ in 0..nw {
                let w = rd_u64(data, off).ok_or("trunc bq_word")?;
                off += 8;
                words.push(w);
            }
            bq_entries.push((sk, words));
        }
        // coerência de era: dims declaradas ⇔ counts (o fast-mount recusa um
        // snapshot onde os dois lados discordam — é corrupção, não política).
        if indexed_dims.len() != corpus_counts.len() {
            return Err("dims/counts mismatch");
        }
        let mut dims_set = BTreeSet::new();
        for &d in &indexed_dims {
            dims_set.insert(d);
        }
        for &(d, _) in &corpus_counts {
            if !dims_set.contains(&d) {
                return Err("count dim not in dims");
            }
        }
        Ok(IndexSnapshot {
            art_keys,
            entity_index,
            indexed_dims,
            corpus_counts,
            bq_words_per_vec,
            bq_live,
            fingerprint,
            written_at,
            lexical_postings,
            lexical_doc_len,
            lexical_n_docs,
            bq_entries,
        })
    }
}

fn rd_u16(data: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(off..off + 2)?.try_into().ok()?))
}
fn rd_u32(data: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?))
}
fn rd_u64(data: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(off..off + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn idx1_roundtrip() {
        let s = IndexSnapshot {
            art_keys: vec![
                "md/L4/fp/0".into(),
                "md/L2/fp/0".into(),
                "rel/causes/a#b".into(),
            ],
            entity_index: vec![
                ("ent/a".into(), vec!["md/L4/fp/0".into()]),
                ("ent/b".into(), vec!["md/L4/fp/0".into(), "md/L4/x".into()]),
            ],
            indexed_dims: vec![4, 256],
            corpus_counts: vec![(4, 10), (256, 2)],
            bq_words_per_vec: 64,
            bq_live: 12,
            fingerprint: 0xdead_beef_cafe,
            written_at: 1234,
            lexical_postings: vec![(
                "corpo".into(),
                vec![("md/L4/fp/0".into(), 1u32), ("md/L2/x".into(), 3u32)],
            )],
            lexical_doc_len: vec![("md/L4/fp/0".into(), 5u32)],
            lexical_n_docs: 2,
            bq_entries: vec![("md/L4/fp/0".into(), vec![0b1010u64, 3u64])],
        };
        let enc = s.encode();
        assert_eq!(&enc[0..4], IDX_MAGIC);
        let d = IndexSnapshot::decode(&enc).unwrap();
        assert_eq!(d, s);
    }

    #[test]
    fn idx1_empty_is_canonical() {
        let s = IndexSnapshot {
            art_keys: vec![],
            entity_index: vec![],
            indexed_dims: vec![],
            corpus_counts: vec![],
            bq_words_per_vec: 0,
            bq_live: 0,
            fingerprint: 0,
            written_at: 0,
            lexical_postings: vec![],
            lexical_doc_len: vec![],
            lexical_n_docs: 0,
            bq_entries: vec![],
        };
        let d = IndexSnapshot::decode(&s.encode()).unwrap();
        assert_eq!(d, s);
    }

    #[test]
    fn idx1_decode_hostile_input_never_panics() {
        for t in [
            &b""[..],
            &b"IDX1"[..],
            &b"IDX1\x01"[..],
            &b"XXXXXXXXXXXXXXXXXXXXXX"[..],
            &b"IDX1\x01\xff\xff\xff\xff"[..],
        ] {
            let _ = IndexSnapshot::decode(t);
        }
        // truncation a partir de um encode válido
        let s = IndexSnapshot {
            art_keys: vec!["md/L4/k".into()],
            entity_index: vec![("ent/x".into(), vec!["md/L4/k".into()])],
            indexed_dims: vec![8],
            corpus_counts: vec![(8, 1)],
            bq_words_per_vec: 1,
            bq_live: 1,
            fingerprint: 7,
            written_at: 8,
            lexical_postings: vec![("corpo".into(), vec![("md/L4/k".into(), 2u32)])],
            lexical_doc_len: vec![("md/L4/k".into(), 4u32)],
            lexical_n_docs: 1,
            bq_entries: vec![("md/L4/k".into(), vec![7u64])],
        };
        let enc = s.encode();
        for cut in 0..enc.len() {
            let _ = IndexSnapshot::decode(&enc[..cut]);
        }
        // dims/counts divergentes = corrupção (não panica, não monta)
        let mut bad = s.clone();
        bad.corpus_counts = vec![(8, 1), (16, 1)];
        let bad_enc = bad.encode();
        // re-encode manual: decode direto falha na coerência
        assert!(IndexSnapshot::decode(&bad_enc).is_err() || {
            // o encode acima é válido (4==2 seria falsamente coerente só se
            // dims mudasse junto); força o mismatch de comprimento:
            let mut bad2 = s;
            bad2.indexed_dims = vec![8, 16];
            bad2.corpus_counts = vec![(8, 1)];
            IndexSnapshot::decode(&bad2.encode()).is_err()
        });
    }

    /// Chave acima do MAX_KLEN no wire → erro, nunca alocação gigante
    /// (mesma disciplina de bounds de `limits.rs`).
    #[test]
    fn idx1_oversized_key_rejected() {
        // encoda manualmente um klen maior que MAX_KLEN
        let mut enc = Vec::new();
        enc.extend_from_slice(IDX_MAGIC);
        enc.push(IDX_VERSION);
        enc.extend_from_slice(&0u64.to_le_bytes()); // fp
        enc.extend_from_slice(&0u64.to_le_bytes()); // ts
        enc.extend_from_slice(&0u32.to_le_bytes()); // wpv
        enc.extend_from_slice(&0u64.to_le_bytes()); // live
        enc.extend_from_slice(&0u32.to_le_bytes()); // ndims
        enc.extend_from_slice(&0u32.to_le_bytes()); // ncounts
        enc.extend_from_slice(&1u32.to_le_bytes()); // nkeys = 1
        enc.extend_from_slice(&((crate::limits::MAX_KLEN + 1) as u16).to_le_bytes());
        assert!(IndexSnapshot::decode(&enc).is_err());
    }
}
