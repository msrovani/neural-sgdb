//! ANN local (v1.1.15 §4 P1): IVF-Flat + HNSW-lite, `no_std` + zero-dep.
//!
//! O BQ flat-scan é O(N) — suficiente até ~10k docs. Acima disso o recall
//! precisa de poda sub-linear antes do rescore FP32. Estes índices são
//! DERIVADOS (rebuild do zero, como ART/BQ/lexical): o storage `md/` continua
//! sendo a fonte da verdade; `Sgdb::recall_ann` constrói sob demanda.
//!
//! Determinísticos (sem RNG): k-means init = primeiros vetores espaçados,
//! níveis HNSW = trailing-zeros do FNV-1a do id. `f32::sqrt` não existe no
//! core bare-metal — distâncias usam L2² (ordem idêntica).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

fn l2_sq(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut s = 0.0f32;
    for i in 0..n {
        let d = a[i] - b[i];
        s += d * d;
    }
    s
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// IVF-Flat: `nlist` centróides (k-means determinístico, 8 iterações) +
/// listas invertidas com vetores completos (rescore exato, sem PQ).
#[derive(Clone, Debug, Default)]
pub struct IvfFlat {
    dim: usize,
    centroids: Vec<Vec<f32>>,
    lists: Vec<Vec<(u64, Vec<f32>)>>,
}

impl IvfFlat {
    pub fn new() -> Self {
        Self::default()
    }

    /// Treina nos vetores `(id, vec)`. `nlist` clamped a `[1, N]`.
    /// Vazio = índice vazio (search devolve vazio, nunca panic).
    pub fn train(&mut self, vecs: &[(u64, Vec<f32>)], nlist: usize) {
        self.dim = vecs.first().map(|(_, v)| v.len()).unwrap_or(0);
        if vecs.is_empty() || self.dim == 0 {
            self.centroids.clear();
            self.lists.clear();
            return;
        }
        let nl = nlist.max(1).min(vecs.len());
        // init espaçado determinístico (cobre clusters sem RNG)
        let step = (vecs.len() / nl).max(1);
        self.centroids = (0..nl)
            .map(|i| vecs[(i * step).min(vecs.len() - 1)].1.clone())
            .collect();
        let mut assign = alloc::vec![0usize; vecs.len()];
        for _ in 0..8 {
            let mut changed = false;
            for (vi, (_, v)) in vecs.iter().enumerate() {
                let mut best = 0usize;
                let mut bd = f32::MAX;
                for (ci, c) in self.centroids.iter().enumerate() {
                    let d = l2_sq(v, c);
                    if d < bd {
                        bd = d;
                        best = ci;
                    }
                }
                if assign[vi] != best {
                    assign[vi] = best;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
            // recompute means
            let mut sums: Vec<Vec<f32>> = alloc::vec![alloc::vec![0.0; self.dim]; nl];
            let mut counts = alloc::vec![0usize; nl];
            for (vi, (_, v)) in vecs.iter().enumerate() {
                let c = assign[vi];
                counts[c] += 1;
                for (d, sv) in sums[c].iter_mut().enumerate().take(self.dim) {
                    *sv += v[d];
                }
            }
            for c in 0..nl {
                if counts[c] == 0 {
                    continue;
                }
                let n = counts[c] as f32;
                for (d, cv) in self.centroids[c].iter_mut().enumerate().take(self.dim) {
                    *cv = sums[c][d] / n;
                }
            }
        }
        self.lists = (0..nl).map(|_| Vec::new()).collect();
        for (vi, (id, v)) in vecs.iter().enumerate() {
            self.lists[assign[vi]].push((*id, v.clone()));
        }
    }

    /// Busca: `nprobe` listas mais próximas do centróide, rescore L2 exato.
    pub fn search(&self, query: &[f32], k: usize, nprobe: usize) -> Vec<(u64, f32)> {
        if self.centroids.is_empty() || query.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut order: Vec<(usize, f32)> = self
            .centroids
            .iter()
            .enumerate()
            .map(|(i, c)| (i, l2_sq(query, c)))
            .collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        let np = nprobe.max(1).min(order.len());
        let mut cand: Vec<(u64, f32)> = Vec::new();
        for (ci, _) in order.into_iter().take(np) {
            for (id, v) in &self.lists[ci] {
                cand.push((*id, l2_sq(query, v)));
            }
        }
        cand.sort_by(|a, b| a.1.total_cmp(&b.1));
        cand.truncate(k.max(1));
        cand
    }

    pub fn len_lists(&self) -> usize {
        self.lists.iter().map(|l| l.len()).sum()
    }
}

/// HNSW-lite: grafo navegável com níveis determinísticos.
/// `level(id) = trailing_zeros(FNV-1a(id))` capped a `max_level` (default 3),
/// vizinhos `M=8` por nó (bidirecional, poda por L2). Busca gulosa por nível
/// + beam `ef` no nível 0. Suficiente como poda ANN antes do rescore FP32.
#[derive(Clone, Debug, Default)]
pub struct HnswLite {
    dim: usize,
    max_level: usize,
    m: usize,
    nodes: BTreeMap<u64, Vec<f32>>,
    levels: BTreeMap<u64, usize>,
    edges: BTreeMap<u64, Vec<u64>>,
    entry: Option<u64>,
}

impl HnswLite {
    pub fn new() -> Self {
        Self {
            dim: 0,
            max_level: 3,
            m: 8,
            nodes: BTreeMap::new(),
            levels: BTreeMap::new(),
            edges: BTreeMap::new(),
            entry: None,
        }
    }

    fn level_of(id: u64, max_level: usize) -> usize {
        let h = fnv1a64(&id.to_le_bytes());
        (h.trailing_zeros() as usize).min(max_level)
    }

    /// Insere `(id, vec)` com conexões aos `M` vizinhos mais próximos
    /// (bidirecional, poda a `2M` por L2). Upsert: re-insere vizinhança.
    pub fn insert(&mut self, id: u64, vec: Vec<f32>) {
        if vec.is_empty() {
            return;
        }
        if self.dim == 0 {
            self.dim = vec.len();
        }
        if vec.len() != self.dim {
            return; // era distinta — ignora (guards S1/era decidem no Sgdb)
        }
        let lv = Self::level_of(id, self.max_level);
        self.nodes.insert(id, vec);
        self.levels.insert(id, lv);
        // vizinhos: M mais próximos entre nós existentes (scan O(N) no build;
        // busca é sub-linear — trade-off documentado do lite)
        let mut near: Vec<(u64, f32)> = Vec::new();
        let me = self.nodes.get(&id).cloned().unwrap_or_default();
        for (oid, ov) in self.nodes.iter() {
            if *oid == id {
                continue;
            }
            near.push((*oid, l2_sq(&me, ov)));
        }
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        near.truncate(self.m);
        let mine: Vec<u64> = near.iter().map(|(o, _)| *o).collect();
        self.edges.insert(id, mine.clone());
        for (oid, _) in near {
            let entry = self.edges.entry(oid).or_default();
            if !entry.contains(&id) {
                entry.push(id);
            }
            // poda a 2M pelos mais próximos
            if entry.len() > self.m * 2 {
                let nb = entry.clone();
                let mut scored: Vec<(u64, f32)> = nb
                    .into_iter()
                    .filter_map(|n| self.nodes.get(&n).map(|v| (n, l2_sq(self.nodes.get(&oid).unwrap(), v))))
                    .collect();
                scored.sort_by(|a, b| a.1.total_cmp(&b.1));
                scored.truncate(self.m * 2);
                *entry = scored.into_iter().map(|(n, _)| n).collect();
            }
        }
        let _ = mine.len();
        // entry = maior nível (desempate menor id)
        let mut best: Option<(usize, u64)> = None;
        for (nid, l) in self.levels.iter() {
            let cand = (*l, *nid);
            let better = match best {
                None => true,
                Some((bl, bn)) => cand.0 > bl || (cand.0 == bl && cand.1 < bn),
            };
            if better {
                best = Some(cand);
            }
        }
        self.entry = best.map(|(_, n)| n);
    }

    /// Busca gulosa: desce do entry pelos vizinhos, beam `ef` no nível 0.
    pub fn search(&self, query: &[f32], k: usize, ef: usize) -> Vec<(u64, f32)> {
        let entry = match self.entry {
            Some(e) => e,
            None => return Vec::new(),
        };
        if query.is_empty() || k == 0 {
            return Vec::new();
        }
        let ef = ef.max(k.max(1)).min(self.nodes.len().max(1));
        // greedy do entry
        let mut cur = entry;
        let mut cur_d = self.nodes.get(&cur).map(|v| l2_sq(query, v)).unwrap_or(f32::MAX);
        loop {
            let mut improved = false;
            if let Some(nb) = self.edges.get(&cur) {
                for n in nb.clone() {
                    if let Some(v) = self.nodes.get(&n) {
                        let d = l2_sq(query, v);
                        if d < cur_d {
                            cur_d = d;
                            cur = n;
                            improved = true;
                        }
                    }
                }
            }
            if !improved {
                break;
            }
        }
        // beam ao redor do ponto de pouso (BFS limitada a ef expansões)
        let mut best: BTreeMap<u64, f32> = BTreeMap::new();
        let mut frontier: Vec<u64> = alloc::vec![cur];
        best.insert(cur, cur_d);
        let mut expanded = 0usize;
        while !frontier.is_empty() && expanded < ef {
            frontier.sort_by(|a, b| {
                best.get(a)
                    .unwrap_or(&f32::MAX)
                    .total_cmp(best.get(b).unwrap_or(&f32::MAX))
            });
            let n = frontier.remove(0);
            expanded += 1;
            if let Some(nb) = self.edges.get(&n).cloned() {
                for m in nb {
                    if best.contains_key(&m) {
                        continue;
                    }
                    if let Some(v) = self.nodes.get(&m) {
                        let d = l2_sq(query, v);
                        best.insert(m, d);
                        frontier.push(m);
                    }
                }
            }
        }
        let mut v: Vec<(u64, f32)> = best.into_iter().collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v.truncate(k.max(1));
        v
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster_vecs() -> Vec<(u64, Vec<f32>)> {
        // 3 clusters 4d bem separados + ruído determinístico
        let mut out = Vec::new();
        let mut id = 1u64;
        for c in 0..3 {
            for i in 0..20 {
                let base = c as f32 * 10.0;
                out.push((
                    id,
                    alloc::vec![
                        base + (i as f32 * 0.01),
                        base + (i as f32 * 0.02),
                        (i as f32 * 0.01),
                        (i as f32 * 0.005),
                    ],
                ));
                id += 1;
            }
        }
        out
    }

    #[test]
    fn ivf_recovers_own_cluster_member() {
        let vecs = cluster_vecs();
        let mut ivf = IvfFlat::new();
        ivf.train(&vecs, 3);
        assert_eq!(ivf.len_lists(), 60);
        let q = vecs[5].1.clone();
        let hits = ivf.search(&q, 3, 1);
        assert!(!hits.is_empty());
        // o próprio vetor deve estar no top-3 (dist 0)
        assert_eq!(hits[0].0, vecs[5].0);
    }

    #[test]
    fn ivf_empty_never_panics() {
        let ivf = IvfFlat::new();
        assert!(ivf.search(&[1.0, 0.0], 5, 2).is_empty());
        let mut ivf2 = IvfFlat::new();
        ivf2.train(&[], 4);
        assert!(ivf2.search(&[1.0], 1, 1).is_empty());
    }

    #[test]
    fn hnsw_greedy_finds_near_member() {
        let vecs = cluster_vecs();
        let mut h = HnswLite::new();
        for (id, v) in &vecs {
            h.insert(*id, v.clone());
        }
        assert_eq!(h.len(), 60);
        let q = vecs[40].1.clone();
        let hits = h.search(&q, 3, 16);
        assert!(!hits.is_empty());
        // membro do mesmo cluster (ids 41-60) no top-3
        assert!((41..=60).contains(&hits[0].0), "top={:?}", hits);
    }

    #[test]
    fn hnsw_empty_and_dim_mismatch_safe() {
        let h = HnswLite::new();
        assert!(h.search(&[1.0], 5, 8).is_empty());
        let mut h2 = HnswLite::new();
        h2.insert(1, alloc::vec![1.0, 0.0]);
        h2.insert(2, alloc::vec![1.0]); // dim distinta → ignorado
        assert_eq!(h2.len(), 1);
    }
}
