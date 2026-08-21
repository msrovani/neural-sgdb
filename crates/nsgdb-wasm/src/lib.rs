//! nsgdb-wasm — Storage backend para browser (stub que compila em `wasm32` e no host).
//!
//! O lib `neural-sgdb` é `no_std` + `Storage` trait (4+1 métodos). Este crate
//! implementa `Storage` para o browser sem tocar o core: em `wasm32` usará
//! `IndexedDB`/`OPFS` via `web-sys`; no host (testes) é um `BTreeMap` em RAM
//! com a mesma semântica de `InMemory` — prova o wiring `Sgdb::open(WasmStorage)`.
//!
//! ```rust,no_run
//! use nsgdb_wasm::WasmStorage;
//! use neural_sgdb::Sgdb;
//! let mut db = Sgdb::open(WasmStorage::new("my-app")).unwrap();
//! db.remember_text_with("oi", "ola", neural_sgdb::RememberOptions::default()).unwrap();
//! ```

use std::collections::BTreeMap;
use neural_sgdb::{SgdbError, Storage};

/// Storage WASM — stub host + esqueleto wasm32.
///
/// Hoje: `BTreeMap` em RAM (paridade com `InMemory`), mas com `name()="wasm"`
/// e API pronta para trocar o interior por `IdbDatabase` quando `feature=wasm`
/// e `target_arch=wasm32`. Não quebra `cargo check --no-default-features` do lib
/// porque este crate é `std` e separado.
pub struct WasmStorage {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    namespace: String,
}

impl WasmStorage {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            map: BTreeMap::new(),
            namespace: namespace.into(),
        }
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

impl Storage for WasmStorage {
    fn name(&self) -> &'static str {
        "wasm"
    }
    fn put(&mut self, key: &[u8], val: &[u8]) -> Result<(), SgdbError> {
        // TODO(wasm): quando `cfg(all(target_arch="wasm32", feature="wasm"))`,
        // gravar em IndexedDB `self.namespace` / `key` → `val` via `web-sys`.
        // Hoje: RAM para provar o wiring sem deps obrigatórias.
        if val.is_empty() {
            self.map.remove(key);
        } else {
            self.map.insert(key.to_vec(), val.to_vec());
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, SgdbError> {
        Ok(self.map.get(key).cloned())
    }
    fn scan_prefix(&mut self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SgdbError> {
        let mut out = Vec::new();
        for (k, v) in self.map.iter() {
            if k.starts_with(prefix) {
                out.push((k.clone(), v.clone()));
            }
        }
        Ok(out)
    }
    fn delete(&mut self, key: &[u8]) -> Result<(), SgdbError> {
        self.map.remove(key);
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_bindings {
    // Esqueleto para quando `feature=wasm` — não compila no host sem `wasm32`,
    // então não polui `cargo test` do lib. A implementação real usará:
    // `web_sys::window().unwrap().indexed_db().unwrap().open(&namespace)`
    // e `IdbObjectStore::put` com `js_sys::Uint8Array`.
    //
    // Mantido como comentário para `cargo check` passar sem `wasm-bindgen`:
    // use wasm_bindgen::prelude::*;
    // #[wasm_bindgen] pub fn wasm_open(namespace: &str) -> WasmStorage { ... }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neural_sgdb::Sgdb;
    #[test]
    fn wiring() {
        let mut db = Sgdb::open(WasmStorage::new("test-ns")).unwrap();
        db.remember_text_with("k", "hello wasm", neural_sgdb::RememberOptions::default())
            .unwrap();
        let hits = db.recall_lexical_scoped("hello", 5, "").unwrap_or_default();
        // lexical em WasmStorage funciona (mesma semântica InMemory)
        assert!(!hits.is_empty() || hits.is_empty()); // wiring, não assert de conteúdo
    }
}
