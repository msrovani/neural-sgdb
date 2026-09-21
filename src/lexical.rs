//! Path lexical contextual (#7, Anthropic "Contextual Retrieval"): índice
//! invertido BM25-style que complementa o recall semântico BQ — recupera
//! casamentos exatos de string / termos raros que o sign-BQ perde (dims baixas,
//! ruído, sinônimos ausentes). Apenas `alloc` (no_std-safe), zero deps.

use crate::fingerprint::{fp_mix_str, fp_mix_u64};
// `ln` do BM25: polyfill no_std consolidado em `crate::math` (v1.1.22).
use crate::math::ln_f32;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Tokeniza em termos alfanuméricos lowercased (sem stopwords/deps).
/// `pub(crate)` — o rerank por ancoragem lexical (v1.1.6 item 4) reusa.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            cur.push(c.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Wrapper `no_std` do tokenizer para o seam `Reranker` (v1.1.14).
pub fn tokenize_for_rerank(text: &str) -> Vec<String> {
    tokenize(text)
}

/// Índice invertido: termo → (storage_key → freq), mais comprimentos.
#[derive(Default)]
pub struct LexicalIndex {
    postings: BTreeMap<String, BTreeMap<String, u32>>,
    doc_len: BTreeMap<String, u32>,
    n_docs: u32,
}

impl LexicalIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexa o texto de um doc (payload L2/L3 — o "companion" da memória).
    /// Upsert: se a chave já foi indexada (overwrite em sessão), remove antes
    /// para não duplicar a contagem.
    pub fn add(&mut self, key: &str, text: &str) {
        if self.doc_len.contains_key(key) {
            self.remove(key);
        }
        let toks = tokenize(text);
        if toks.is_empty() {
            return;
        }
        self.n_docs = self.n_docs.saturating_add(1);
        self.doc_len.insert(String::from(key), toks.len() as u32);
        let mut tf: BTreeMap<String, u32> = BTreeMap::new();
        for t in &toks {
            *tf.entry(t.clone()).or_insert(0) += 1;
        }
        for (t, f) in tf {
            self.postings.entry(t).or_default().insert(String::from(key), f);
        }
    }

    /// Remove um doc (delete/rebuild). Decrementa n_docs; postings esvaziadas
    /// de termos são removidas.
    pub fn remove(&mut self, key: &str) {
        let Some(n) = self.doc_len.remove(key) else {
            return;
        };
        // recontagem simples: subtrai o doc removido (n_docs >= 1 garantido)
        self.n_docs = self.n_docs.saturating_sub(1);
        // remove o key de todos os postings (varredura — rebuild é o comum)
        let mut empty = Vec::new();
        for (term, plist) in self.postings.iter_mut() {
            plist.remove(key);
            if plist.is_empty() {
                empty.push(term.clone());
            }
        }
        for t in empty {
            self.postings.remove(&t);
        }
        let _ = n;
    }

    pub fn len(&self) -> usize {
        self.n_docs as usize
    }

    /// Mistura o estado canônico do índice num hash FNV-1a (v1.1.21, ADR-0011).
    ///
    /// Ordem canônica por construção: `postings` é `BTreeMap<term, BTreeMap<doc,
    /// tf>>` — termo asc, doc asc. A ordem de INSERÇÃO não afeta o resultado
    /// (o `BTreeMap` já ordena), que é exatamente o invariante que o fingerprint
    /// precisa provar. Custo O(total de postings), sem alloc.
    pub(crate) fn fp_mix_into(&self, mut h: u64) -> u64 {
        h = fp_mix_u64(h, self.n_docs as u64);
        h = fp_mix_u64(h, self.postings.len() as u64);
        for (term, docs) in &self.postings {
            h = fp_mix_str(h, term);
            h = fp_mix_u64(h, docs.len() as u64);
            for (doc, tf) in docs {
                h = fp_mix_str(h, doc);
                h = fp_mix_u64(h, *tf as u64);
            }
        }
        h
    }

    pub fn is_empty(&self) -> bool {
        self.n_docs == 0
    }

    /// BM25-ish: log-tf × idf, soma por termo da query. Retorna (key, score,
    /// termos da query que casaram) desc (determinístico por key no empate).
    /// Os termos casados são o grounding do hit (v1.1.6) — o consumidor vê o
    /// "porquê" do casamento sem re-tokenizar.
    #[inline]
    pub fn search(&self, query: &str, k: usize) -> Vec<(String, f32, Vec<String>)> {
        let toks = tokenize(query);
        // dedup + cap (DoS bound 10): query gigante (1MiB tokenizada ~100k termos) sem cap aloca O(N)
        let mut uniq: BTreeMap<String, ()> = BTreeMap::new();
        for t in toks {
            if uniq.len() >= 1024 {
                break;
            }
            uniq.insert(t, ());
        }
        let mut scores: BTreeMap<String, f32> = BTreeMap::new();
        let mut matched: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let n = self.n_docs.max(1) as f32;
        for t in uniq.keys() {
            let Some(plist) = self.postings.get(t) else {
                continue;
            };
            let df = plist.len() as f32;
            let idf = ln_f32((n + 1.0) / (df + 1.0)) + 1.0;
            for (key, f) in plist {
                let tf = 1.0 + ln_f32(*f as f32);
                *scores.entry(key.clone()).or_insert(0.0) += tf * idf;
                matched.entry(key.clone()).or_default().push(t.clone());
            }
        }
        let mut out: Vec<(String, f32, Vec<String>)> = scores
            .into_iter()
            .map(|(k, s)| (k.clone(), s, matched.remove(&k).unwrap_or_default()))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out.truncate(k);
        out
    }

    /// Fast path sem `matched_terms` — para rerank interno e oversample onde o
    /// grounding não é necessário. Evita alloc de `Vec<String>` por hit (MG2).
    #[inline]
    pub fn search_fast(&self, query: &str, k: usize) -> Vec<(String, f32)> {
        let toks = tokenize(query);
        let mut uniq: BTreeMap<String, ()> = BTreeMap::new();
        for t in toks {
            if uniq.len() >= 1024 {
                break;
            }
            uniq.insert(t, ());
        }
        let mut scores: BTreeMap<String, f32> = BTreeMap::new();
        let n = self.n_docs.max(1) as f32;
        for t in uniq.keys() {
            let Some(plist) = self.postings.get(t) else {
                continue;
            };
            let df = plist.len() as f32;
            let idf = ln_f32((n + 1.0) / (df + 1.0)) + 1.0;
            for (key, f) in plist {
                let tf = 1.0 + ln_f32(*f as f32);
                *scores.entry(key.clone()).or_insert(0.0) += tf * idf;
            }
        }
        let mut out: Vec<(String, f32)> = scores.into_iter().collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(core::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out.truncate(k);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard de REGRESSÃO da consolidação de math (v1.1.22): o `ln_f32` foi
    /// MOVIDO para `crate::math` sem tocar no algoritmo — e `ln_f32` alimenta o
    /// IDF/TF do BM25, logo mexer nele move scores, que movem o ranking, que
    /// move o texto do hot test. O que prova "sem tocar" não é o diff: é este
    /// ranking congelado. Se ele mudar, a precisão do polyfill mudou.
    #[test]
    fn bm25_ranking_is_frozen_across_math_move() {
        use alloc::vec;
        let mut idx = LexicalIndex::new();
        idx.add("md/L2/a", "rust memory database agent recall");
        idx.add("md/L2/b", "python web framework routing");
        idx.add("md/L2/c", "rust agent memory recall recall");
        idx.add("md/L2/d", "unrelated filler text here");
        let got = idx.search("rust memory recall", 4);
        let keys: Vec<&str> = got.iter().map(|(k, _, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            vec!["md/L2/c", "md/L2/a"],
            "ranking BM25 mudou — a precisao do polyfill mudou: {got:?}"
        );
        // c vence a: mais termos raros + repeticao (TF) — e `ln_f32` entra
        // tanto no IDF quanto no TF, entao a ordem E os scores pinam o polyfill.
        let want = [5.614_191_f32, 4.560_493_5_f32];
        for ((_, score, _), w) in got.iter().zip(want.iter()) {
            let rel = ((score - w) / w).abs();
            assert!(rel < 1e-6, "score BM25 mudou: got={score} want={w}");
        }
        // b e d nao casam termo nenhum: BM25 exige match, nao cortesia.
        assert_eq!(got.len(), 2, "{got:?}");
    }
}
