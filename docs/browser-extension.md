# Extensão de navegador — nsgdb local-first

> Memória do uso do navegador, 100% local, sem nuvem, sem API key.
> Mesmo prático do `mem0`/`Supermemory` (1 clique), mas verbatim + IndexedDB.
> Complementa `docs/adr/0008-local-embedder.md:62` Other uses e `crates/nsgdb-wasm`.

## O que faz

A extensão captura **automaticamente** o uso do navegador e guarda como memória verbatim
(`md/L3/` léxico via `WasmStorage` → `IndexedDB` `crates/nsgdb-wasm/src/lib.rs`):

- URL + título + trecho `document.body.innerText.slice(0,2000)` de cada página completa
- `chrome.history.onVisited` + `chrome.tabs.onUpdated` (evento `complete`)
- Salva com `Sgdb::remember_text_with(url, trecho, {scope:"browser", entities:["page/url"]})`
  (`src/sgdb.rs:remember_text_with` — L3 sem vetor, sem abrir era BQ)
- Recall via popup: `recall_lexical_scoped("café", 5, "browser")` ou `recall_temporal` com `at=agora`

Diferença para `mem0` (`docs/memory-landscape.md:22`): `mem0` extrai entidades e manda para nuvem;
nsgdb guarda **verbatim** local (estilo `mempalace` `docs/memory-landscape.md:30`) — léxico acha sem embedder,
`nsgdb-embed` (`crates/nsgdb-embed`) só entra se quiser semântico.

## Instalação (usuário iniciante, 1 minuto, sem terminal)

1. Baixe o zip `nsgdb-ext.zip` em Releases (ou clone a pasta `extension/` deste repo)
2. Descompacte (ex: `Área de Trabalho/nsgdb-ext`)
3. Chrome/Edge → barra: `chrome://extensions`
4. Ligue `Modo do desenvolvedor` (canto superior direito)
5. `Carregar sem compactação` → selecione a pasta `nsgdb-ext`
6. Ícone nsgdb aparece na barra

> Futuro: Chrome Web Store (1 clique). Hoje `Load unpacked` já é 1 clique prático igual ao `mem0` store.

## Permissões (por que cada uma)

`manifest.json` (MV3):

```json
{
  "manifest_version": 3,
  "name": "nsgdb memory",
  "permissions": ["history", "tabs", "storage", "scripting"],
  "host_permissions": ["<all_urls>"],
  "background": { "service_worker": "background.js" },
  "action": { "default_popup": "popup.html" }
}
```

- `history` — `onVisited` (URL + título)
- `tabs` + `scripting` — `content script` para `innerText`
- `storage` — `WasmStorage` namespace `ext-ns` em `IndexedDB` (quando `feature=wasm`; hoje `BTreeMap` RAM no host `crates/nsgdb-wasm/src/lib.rs:18`)
- Dados **nunca saem da máquina** (local-first, `no_std` + `Storage` trait `src/storage.rs`)

## Como usa (sem digitar)

Após instalar, navegue normalmente. A extensão guarda sozinha.

Popup → busca: `o que vi sobre café ontem?` → `recall_lexical_scoped("café", 5, "browser")` lista as páginas
com `Hit { key, text, dist, path=lexical, matched_terms }` (`src/ctype.rs`).

Limpar: `chrome.storage.local.clear` + `db` `scan_prefix("md/L3/")` + `delete` (físico, `src/sgdb.rs:delete`).

## Para desenvolvedor (build wasm real)

Hoje o stub já compila: `cargo test --manifest-path crates/nsgdb-wasm/Cargo.toml`.

Para persistência real:

```bash
wasm-pack build crates/nsgdb-wasm --target web --features wasm
# background.js troca:
# import { WasmStorage } from './pkg/nsgdb_wasm.js';
# const db = Sgdb.open(new WasmStorage("ext-ns")); // IndexedDB
```

Sem quebrar contratos: `nsgdb-wasm` é crate host (`std`, `wasm-bindgen` só com `feature=wasm`), lib continua `no_std` zero deps (`src/lib.rs`).

## Ver também

- `crates/nsgdb-embed` — embedder local 384-dim stub (`cargo run --manifest-path crates/nsgdb-embed/Cargo.toml --example demo`)
- `examples/host_scheduler.rs` — governa `expire_old`/`decay`/`consolidate` periódico
- `BENCH_COMPARE_1.1.10_vs_1.1.11.md` — ganhos funcionais > bench
