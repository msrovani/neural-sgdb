# nsgdb browser extension — 1-clique local-first

Instale em 1 minuto sem terminal (mesmo prático do mem0, mas sem nuvem):

1. `chrome://extensions` → ligue `Modo do desenvolvedor`
2. `Carregar sem compactação` → selecione a pasta `extension/`
3. Navegue — cada página vira `md/L3/<hash>` verbatim em `WasmStorage` (`crates/nsgdb-wasm`)
4. Clique no ícone → busque `café` → `recall_lexical_scoped("café",5,"browser")`

Permissões: `history` (onVisited), `tabs`+`scripting` (innerText 2k), `storage` (IndexedDB).

Para Chrome Web Store (OBRIGATÓRIO a cada nova versão — `VERSIONING.md` passo 6): `zip -r nsgdb-ext.zip extension/` → upload em https://chrome.google.com/webstore/devconsole (1 clique para usuário final). Atualize `manifest.json` `version` para a mesma `Cargo.toml` antes de enviar.

Build wasm real: `wasm-pack build crates/nsgdb-wasm --target web --features wasm` e troque `background.js` RAM `Map` por `WasmStorage` IndexedDB.
