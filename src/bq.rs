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
        if w == 0 || self.ids.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(u64, u32)> = Vec::with_capacity(self.ids.len());
        for (i, id) in self.ids.iter().enumerate() {
            let start = i * w;
            let vec = &self.flat[start..start + w];
            scored.push((*id, hamming(query, vec)));
        }
        scored.sort_by_key(|(_, d)| *d);
        scored.truncate(k);
        scored
    }

    pub fn top_k_f32(&self, query: &[f32], k: usize) -> Vec<(u64, u32)> {
        self.top_k(&quantize_f32(query), k)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
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
}
