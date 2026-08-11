//! ADR-0063 — BQ Flat + Hamming via `hamming_dispatch`.

use alloc::vec;
use alloc::vec::Vec;

use crate::hamming_dispatch::{self, hamming as hamming_dispatch_fn};

pub fn quantize_f32(v: &[f32]) -> Vec<u64> {
    let n_bits = v.len();
    let n_words = (n_bits + 63) / 64;
    let mut out = vec![0u64; n_words];
    for (i, &x) in v.iter().enumerate() {
        if x > 0.0 {
            out[i / 64] |= 1u64 << (i % 64);
        }
    }
    out
}

/// Quantiza com threshold adaptativo da PRÓPRIA query (média) em vez de
/// `> 0` (#4): os bitvecs ARMAZENADOS não mudam (contrato intacto) — só a
/// query é re-centrada. Ajuda quando a query tem offset (ex: +5 em todas as
/// dims): `sign(x)>0` infla todos os bits e perde o sinal; `x > mean(query)`
/// realinha a query à distribuição dos vetores armazenados.
pub fn quantize_f32_centered(v: &[f32]) -> Vec<u64> {
    let n = v.len();
    let mean = if n > 0 { v.iter().sum::<f32>() / n as f32 } else { 0.0 };
    let n_words = (n + 63) / 64;
    let mut out = vec![0u64; n_words];
    for (i, &x) in v.iter().enumerate() {
        if x > mean {
            out[i / 64] |= 1u64 << (i % 64);
        }
    }
    out
}

/// Quantiza para exatamente 16 words (1024 dims) — pad/trunc.
pub fn quantize_f32_1024(v: &[f32]) -> [u64; 16] {
    let q = quantize_f32(v);
    let mut out = [0u64; 16];
    let n = q.len().min(16);
    out[..n].copy_from_slice(&q[..n]);
    out
}

pub fn hamming_path() -> &'static str {
    hamming_dispatch::path_name()
}

#[inline]
pub fn hamming(a: &[u64], b: &[u64]) -> u32 {
    hamming_dispatch_fn(a, b)
}

pub struct BqFlatIndex {
    pub ids: Vec<u64>,
    pub flat: Vec<u64>,
    pub words_per_vec: usize,
}

/// FNV-1a sobre os bytes de um bloco de words (bucket key do MIH).
fn hash_words(words: &[u64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &w in words {
        for b in w.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// **Multi-Index Hashing** (#1, Norouzi et al.) sobre os bitvecs JÁ armazenados
/// — recall-time, formato intocado. Particiona cada código em `blocks` blocos;
/// cada bloco tem uma tabela de buckets (hash do bloco → ids). Query: probe os
/// buckets dos blocos da query, une os candidatos e rankeia por hamming
/// completo. Vira o scan O(N) em O(candidatos) — candidatos ≈ N/2^(bits/bloco)
/// para dados aleatórios; o match exato (mesmos blocos) é sempre recuperado.
pub struct MihIndex {
    blocks: usize,
    block_words: usize,
    words_per_vec: usize,
    /// por bloco: bucket hash → posições no `src.flat` (evita busca O(N) por id)
    buckets: Vec<alloc::collections::BTreeMap<u64, Vec<usize>>>,
}

impl MihIndex {
    pub fn build(src: &BqFlatIndex, blocks: usize) -> Self {
        let blocks = blocks.max(1);
        let w = src.words_per_vec.max(1);
        let block_words = (w + blocks - 1) / blocks;
        let mut buckets: Vec<alloc::collections::BTreeMap<u64, Vec<usize>>> =
            (0..blocks).map(|_| alloc::collections::BTreeMap::new()).collect();
        for (i, _id) in src.ids.iter().enumerate() {
            let start = i * w;
            let vec = &src.flat[start..start + w];
            for j in 0..blocks {
                let lo = (j * block_words).min(w);
                if lo >= w {
                    break; // blocos além da largura do vetor (w < blocks)
                }
                let hi = ((j + 1) * block_words).min(w);
                let key = hash_words(&vec[lo..hi]);
                buckets[j].entry(key).or_default().push(i);
            }
        }
        MihIndex { blocks, block_words, words_per_vec: w, buckets }
    }

    /// Recupera candidatos (posições) : união dos buckets dos blocos da query.
    /// `probes` = bit-flips por bloco p/ aumentar recall (0 = probe exato).
    pub fn candidates(&self, query: &[u64], probes: usize) -> Vec<usize> {
        let mut set: alloc::collections::BTreeSet<usize> = alloc::collections::BTreeSet::new();
        for j in 0..self.blocks {
            let lo = j * self.block_words;
            if lo >= query.len() {
                break;
            }
            let hi = (lo + self.block_words).min(query.len());
            let block = &query[lo..hi];
            // probe exato + até `probes` flips de bit no primeiro word do bloco
            let mut keys = vec![hash_words(block)];
            if probes > 0 && !block.is_empty() {
                for b in 0..probes.min(64) {
                    let mut w = block[0];
                    w ^= 1u64 << b;
                    let mut probe: alloc::vec::Vec<u64> = block.to_vec();
                    probe[0] = w;
                    keys.push(hash_words(&probe));
                }
            }
            for k in keys {
                if let Some(bucket) = self.buckets[j].get(&k) {
                    for &pos in bucket {
                        set.insert(pos);
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    /// Rankeia os candidatos por hamming completo contra o src — top-k
    /// determinístico (dist, id), paridade com `BqFlatIndex::top_k`.
    pub fn top_k(&self, src: &BqFlatIndex, query: &[u64], k: usize, probes: usize) -> Vec<(u64, u32)> {
        if k == 0 {
            return Vec::new();
        }
        let cand = self.candidates(query, probes);
        let mut scored: Vec<(u64, u32)> = Vec::with_capacity(cand.len());
        for &pos in &cand {
            let start = pos * self.words_per_vec;
            let vec = &src.flat[start..start + self.words_per_vec];
            scored.push((src.ids[pos], hamming_dispatch::hamming(query, vec)));
        }
        scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        scored.truncate(k);
        scored
    }

    pub fn top_k_f32(&self, src: &BqFlatIndex, query: &[f32], k: usize, probes: usize) -> Vec<(u64, u32)> {
        self.top_k(src, &quantize_f32(query), k, probes)
    }
}

impl BqFlatIndex {
    pub fn new() -> Self {
        BqFlatIndex {
            ids: Vec::new(),
            flat: Vec::new(),
            words_per_vec: 0,
        }
    }

    pub fn insert(&mut self, id: u64, bits: Vec<u64>) {
        if self.words_per_vec == 0 {
            self.words_per_vec = bits.len().max(1);
        }
        let w = self.words_per_vec;
        self.ids.push(id);
        if bits.len() >= w {
            self.flat.extend_from_slice(&bits[..w]);
        } else {
            self.flat.extend_from_slice(&bits);
            for _ in bits.len()..w {
                self.flat.push(0);
            }
        }
    }

    pub fn insert_f32(&mut self, id: u64, v: &[f32]) {
        self.insert(id, quantize_f32(v));
    }

    pub fn insert_1024(&mut self, id: u64, bits: &[u64; 16]) {
        if self.words_per_vec == 0 {
            self.words_per_vec = 16;
        }
        let w = self.words_per_vec;
        self.ids.push(id);
        // Respeita a largura corrente (bughunt #11): anexar 16 words
        // incondicionalmente com words_per_vec != 16 quebrava a invariante
        // flat.len() == ids.len()*words_per_vec → top_k com slice fora de
        // bounds (panic) ou resultados errados. Trunca/pad a `w` como o insert.
        if w <= 16 {
            self.flat.extend_from_slice(&bits[..w]);
        } else {
            self.flat.extend_from_slice(bits);
            for _ in 16..w {
                self.flat.push(0);
            }
        }
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.flat.clear();
        self.words_per_vec = 0;
    }

    pub fn top_k(&self, query: &[u64], k: usize) -> Vec<(u64, u32)> {
        let w = self.words_per_vec;
        if w == 0 || self.ids.is_empty() || k == 0 {
            return Vec::new();
        }
        let k = k.min(self.ids.len());
        if k >= self.ids.len() {
            // k >= len: heap vira sort completo (mesmo resultado, menor custo
            // de bookkeeping) — mantém determinismo (dist, id).
            let mut scored: Vec<(u64, u32)> = Vec::with_capacity(self.ids.len());
            for (i, id) in self.ids.iter().enumerate() {
                let start = i * w;
                let vec = &self.flat[start..start + w];
                scored.push((*id, hamming(query, vec)));
            }
            scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
            scored
        } else {
            // Bounded top-k: O(N·D/64 + N log k) em vez de O(N·D/64 + N log N).
            // Max-heap de tamanho k: raiz = pior candidato; evict quando um
            // melhor chega. Ordenação final por (dist, id) — determinística.
            let mut heap: alloc::collections::BinaryHeap<Cand> =
                alloc::collections::BinaryHeap::with_capacity(k);
            for (i, id) in self.ids.iter().enumerate() {
                let start = i * w;
                let vec = &self.flat[start..start + w];
                let cand = Cand {
                    dist: hamming(query, vec),
                    id: *id,
                };
                if heap.len() < k {
                    heap.push(cand);
                } else if let Some(worst) = heap.peek() {
                    if cand < *worst {
                        heap.pop();
                        heap.push(cand);
                    }
                }
            }
            heap.into_sorted_vec()
                .into_iter()
                .map(|c| (c.id, c.dist))
                .collect()
        }
    }

    pub fn top_k_f32(&self, query: &[f32], k: usize) -> Vec<(u64, u32)> {
        self.top_k(&quantize_f32(query), k)
    }

    /// top-k com query re-centrada (#4) — alternativa ótima para queries com
    /// offset; os bitvecs armazenados permanecem `sign(x)>0`.
    pub fn top_k_f32_centered(&self, query: &[f32], k: usize) -> Vec<(u64, u32)> {
        self.top_k(&quantize_f32_centered(query), k)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }
}

/// Candidato BQ ordenado por (dist, id) — determinístico. `Ord` normal faz o
/// BinaryHeap (max-heap) ter o PIOR candidato na raiz, permitindo eviction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cand {
    dist: u32,
    id: u64,
}

impl core::cmp::Ord for Cand {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.dist.cmp(&other.dist).then(self.id.cmp(&other.id))
    }
}

impl core::cmp::PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Default for BqFlatIndex {
    fn default() -> Self {
        Self::new()
    }
}

pub fn smoke() -> bool {
    hamming_dispatch::select_best_hamming_kernel();
    let kernel = hamming_dispatch::path_name();
    let mut idx = BqFlatIndex::new();
    idx.insert_f32(1, &[1.0, -1.0, 1.0, -1.0]);
    idx.insert_f32(2, &[-1.0, -1.0, -1.0, -1.0]);
    idx.insert_f32(3, &[1.0, 1.0, 1.0, 1.0]);
    let hits = idx.top_k_f32(&[1.0, -1.0, 1.0, -1.0], 1);
    let hits_ok = hits.len() == 1 && hits[0].0 == 1 && hits[0].1 == 0;
    if !hits_ok {
        crate::sgdb_log!(
            "SGDB BQ top_k FAIL kernel={} len={} id={} dist={}",
            kernel,
            hits.len(),
            hits.first().map(|h| h.0).unwrap_or(99),
            hits.first().map(|h| h.1).unwrap_or(99)
        );
    }
    let s1024 = hamming_dispatch::smoke_1024();
    if !s1024 {
        crate::sgdb_log!("SGDB BQ smoke_1024 FAIL kernel={}", kernel);
    }
    hits_ok && s1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bq_top_k() {
        assert!(smoke());
    }

    #[test]
    fn bq_quantize() {
        let q = quantize_f32(&[1.0, -1.0, 1.0, -1.0]);
        assert_eq!(q, vec![0b0101u64]);
        let q2 = quantize_f32_1024(&[1.0, -1.0]);
        assert_eq!(q2.len(), 16);
        assert_eq!(q2[0], 0b01);
    }

    // ── BQ top-k bounded heap: casos-limite + determinismo (maturation P3) ──

    fn mk_idx(n: usize) -> BqFlatIndex {
        let mut idx = BqFlatIndex::new();
        for i in 0..n {
            // vetores pseudo-aleatórios determinísticos — distâncias variadas
            let v = [1.0, -1.0, (i % 7) as f32 / 3.0 - 1.0, -1.0];
            idx.insert_f32(i as u64, &v);
        }
        idx
    }

    #[test]
    fn top_k_zero_returns_empty() {
        let idx = mk_idx(10);
        assert!(idx.top_k(&[0, 0, 0, 0], 0).is_empty());
    }

    #[test]
    fn top_k_empty_index() {
        let idx = BqFlatIndex::new();
        assert!(idx.top_k(&[0, 0, 0, 0], 5).is_empty());
    }

    #[test]
    fn top_k_ge_len_returns_all_sorted() {
        let idx = mk_idx(10);
        let hits = idx.top_k(&[0, 0, 0, 0], 100); // k >= len
        assert_eq!(hits.len(), 10);
        // ordenado por (dist, id) — distâncias não-decrescentes
        for w in hits.windows(2) {
            assert!(w[0].1 <= w[1].1, "ordem de distância quebrada: {hits:?}");
        }
    }

    #[test]
    fn top_k_deterministic_and_sorted() {
        let idx = mk_idx(50);
        let q = [1u64 << 0, 1 << 1, 1 << 2, 1 << 3];
        let a = idx.top_k(&q, 7);
        let b = idx.top_k(&q, 7);
        assert_eq!(a, b, "top_k não determinístico");
        // ordenação estrita por (dist, id)
        for w in a.windows(2) {
            let (d1, id1) = (w[0].1, w[0].0);
            let (d2, id2) = (w[1].1, w[1].0);
            assert!(
                d1 < d2 || (d1 == d2 && id1 < id2),
                "ordem (dist,id) quebrada: {a:?}"
            );
        }
    }

    #[test]
    fn top_k_tie_break_by_id() {
        // vetores idênticos → mesmo score → tie-break por id (menor primeiro)
        let mut idx = BqFlatIndex::new();
        for i in [5u64, 2, 9, 1] {
            idx.insert_f32(i, &[1.0, 1.0, 1.0, 1.0]);
        }
        let hits = idx.top_k(&[1, 1, 1, 1], 4);
        assert_eq!(hits.len(), 4);
        let ids: Vec<u64> = hits.iter().map(|h| h.0).collect();
        assert_eq!(ids, vec![1, 2, 5, 9], "tie-break por id falhou: {ids:?}");
    }

    #[test]
    fn mih_prunes_candidates_and_recovers_exact() {
        // #1: MIH deve reduzir o pool de candidatos de O(N) para uma fração
        // (candidatos ≈ N/2^(bits/bloco)) e recuperar o match exato (mesmos
        // blocos) com hamming completo.
        let mut src = BqFlatIndex::new();
        let mut vecs = Vec::with_capacity(10_000);
        for i in 0..10_000u64 {
            let mut s = i.wrapping_mul(1103515245).wrapping_add(12345);
            let mut v = vec![0f32; 1024];
            for x in v.iter_mut() {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0;
            }
            vecs.push(v.clone());
            src.insert_f32(i, &v);
        }
        let mih = MihIndex::build(&src, 4); // 4 blocos de 256 bits
        // query = um vetor armazenado (exato nos 4 blocos)
        let q = &vecs[42];
        let cand = mih.candidates(&quantize_f32(q), 0);
        // medição: candidatos << N (com 256 bits/bloco, ~poucos por bucket)
        assert!(
            cand.len() < 10_000 / 8,
            "MIH não reduziu o pool: {} candidatos de 10000",
            cand.len()
        );
        // o exato (hamming 0) está no top-1
        let top = mih.top_k_f32(&src, q, 1, 0);
        assert_eq!(top[0], (42, 0), "match exato deveria estar no top-1 MIH");
        // paridade com o brute-force: o top-1 por hamming completo do MIH
        // coincide com o do BqFlatIndex (query = armazenado)
        let brute = src.top_k_f32(q, 1);
        assert_eq!(top[0].0, brute[0].0);
    }

    #[test]
    fn centered_query_recovers_exact_on_offset_query() {
        // #4: query com offset (todos os componentes +5) — `sign(x)>0` infla
        // todos os bits (hamming empata por id e perde o exato); re-centrar a
        // query pela própria média realinha à distribuição armazenada.
        let mut idx = BqFlatIndex::new();
        // vetores centrados em zero (LCG determinístico)
        for i in 0..2000 {
            let mut s = (i as u64).wrapping_mul(1103515245).wrapping_add(12345);
            let mut v = Vec::with_capacity(16);
            for _ in 0..16 {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                v.push(((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0);
            }
            idx.insert_f32(i as u64, &v);
        }
        // query = vetor 7 + offset +5 em todas as dims
        let target = 7u64;
        let mut q = vec![0f32; 16];
        {
            let mut s = target.wrapping_mul(1103515245).wrapping_add(12345);
            for x in q.iter_mut() {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                *x = ((s >> 32) as i32 % 200) as f32 / 100.0 - 1.0 + 5.0;
            }
        }
        let std_top = idx.top_k_f32(&q, 5);
        let ctr_top = idx.top_k_f32_centered(&q, 5);
        // top_k (vazio) nunca panics
        assert!(!std_top.is_empty() && !ctr_top.is_empty());
        // hamming do exato sob query centrada deve ser <= ao da query padrão
        let ham_std = std_top.iter().position(|(id, _)| *id == target);
        let ham_ctr = ctr_top.iter().position(|(id, _)| *id == target);
        assert!(
            ham_ctr.is_some(),
            "query re-centrada deveria trazer o exato: std={ham_std:?} ctr={ham_ctr:?}"
        );
    }

    #[test]
    fn insert_1024_respects_established_width() {
        // bughunt #11: insert_1024 anexava 16 words INCONDICIONALMENTE, mesmo
        // com words_per_vec já estabelecido em outro valor — quebrava a
        // invariante flat.len() == ids.len()*words_per_vec → top_k com slice
        // fora de bounds (panic) ou resultados errados. Deve respeitar a
        // largura corrente (truncar/pad), como insert/insert_f32.
        // f32 curto primeiro (w=1), depois insert_1024 (deve truncar a 1 word)
        let mut idx = BqFlatIndex::new();
        idx.insert_f32(1, &[1.0, -1.0, 1.0, -1.0]);
        idx.insert_1024(2, &[0u64; 16]);
        assert_eq!(idx.flat.len(), idx.ids.len() * idx.words_per_vec);
        let hits = idx.top_k(&[0], 10);
        assert_eq!(hits.len(), 2);
        // insert_1024 primeiro (w=16), f32 curto depois (deve pad a 16)
        let mut idx2 = BqFlatIndex::new();
        idx2.insert_1024(1, &[0u64; 16]);
        idx2.insert_f32(2, &[1.0, -1.0]);
        assert_eq!(idx2.flat.len(), idx2.ids.len() * idx2.words_per_vec);
        let hits2 = idx2.top_k(&[0u64; 16], 10);
        assert_eq!(hits2.len(), 2);
        // insert wide (w=17) antes, insert_1024 depois (deve pad a 17)
        let mut idx3 = BqFlatIndex::new();
        let mut wide = vec![0u64; 17];
        wide[0] = 1;
        idx3.insert(1, wide);
        idx3.insert_1024(2, &[0u64; 16]);
        assert_eq!(idx3.flat.len(), idx3.ids.len() * idx3.words_per_vec);
        let hits3 = idx3.top_k(&[1u64 << 0], 10);
        assert_eq!(hits3.len(), 2);
        assert_eq!(hits3[0], (1, 0)); // wide com word0=1 → dist 0
    }

    #[test]
    fn top_k_parity_with_full_sort() {
        // heap limitado (k pequeno) == full sort (k = len) para o mesmo top-k
        let idx = mk_idx(64);
        let q = [1u64 << 0, 1 << 1, 1 << 2, 1 << 3];
        let heap_top = idx.top_k(&q, 5);
        let full = idx.top_k(&q, 64); // k >= len → full sort
        let full_top: Vec<(u64, u32)> = full.into_iter().take(5).collect();
        assert_eq!(heap_top, full_top, "heap ≠ full sort no top-5");
    }
}
