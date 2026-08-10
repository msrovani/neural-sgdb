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
        self.ids.push(id);
        self.flat.extend_from_slice(bits);
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
