//! Path lexical contextual (#7, Anthropic "Contextual Retrieval"): índice
//! invertido BM25-style que complementa o recall semântico BQ — recupera
//! casamentos exatos de string / termos raros que o sign-BQ perde (dims baixas,
//! ruído, sinônimos ausentes). Apenas `alloc` (no_std-safe), zero deps.

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

/// `ln` para no_std (f32::ln não existe no core p/ bare-metal — ponytail,
/// como o `sqrt_f32`): expoente IEEE + série no mantissa. Precisão ~1e-5,
/// suficiente p/ ranking BM25 (ordenação, não valor exato).
fn ln_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return -3.0; // clamp: log de 0/neg não usado no BM25
    }
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let mant = (bits & 0x7F_FFFF) | 0x3F80_0000; // [1,2)
    let m = f32::from_bits(mant);
    let y = m - 1.0;
    let ln_m = y * (1.0 - 0.5 * y + y * y / 3.0 - y * y * y / 4.0 + y * y * y * y / 5.0);
    exp as f32 * core::f32::consts::LN_2 + ln_m
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
        // dedup query termos: evita m.contains() O(n) por hit quando query repete
        let mut uniq: BTreeMap<String, ()> = BTreeMap::new();
        for t in toks {
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
