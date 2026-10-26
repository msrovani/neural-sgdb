# Interop — neural-sgdb ↔ neural-os-core (AIOS)

> Origem do substrato: extraído / espelhado a partir do stack **neural-os-core**
> (AIOS). Contrato de fio: **NMD1** + **TKLV** byte-identical. Side-tables
> cognitivas (MDM1) evoluem no standalone sem mudar NMD1/TKLV.

## Premissas que **não** podem partir

| Contrato | Onde |
|----------|------|
| Zero deps na lib; `no_std` + `std` | `Cargo.toml`, gate bare-metal |
| NMD1 layout | `src/memory_doc.rs` — `golden_nmd1_bytes` |
| TKLV / TKCK | `src/tickv.rs` — magic `TKLV`, tombstone `TKL\0` |
| Core não decide; ADD-only | `docs/doctrine.md` |
| Lexical-first MCP | ADR-0008 |

## Evidência de paridade (revalidado 2026-10-19)

Tree OS de referência **nesta máquina**: `C:\Users\Crbm\Videos\neural-os-core`
(`C:\DEV\neural-os-core-latest`, citado antes, não existe aqui — o path estava
stale).

| Artefacto | Standalone (`neural-sgdb`) | OS |
|-----------|----------------------------|-----|
| NMD1 golden L1 `"k"` / `0xAA` | `src/memory_doc.rs::golden_nmd1_bytes` | `crates/k_ai/src/sgdb/memory_doc.rs::golden_nmd1_bytes_match_neural_sgdb` — **mesmo vetor de bytes** |
| META (MDM1) | `META_VERSION = 8` (scope_dims + model_id + authority) | submodule `crates/neural-sgdb` **`META_VERSION = 8`** — mesmo código (pin v1.4.5, `16c0c29`) |
| Crate SemVer | **1.4.5** | **1.4.5** (submodule pinado; era 1.1.13 até 2026-10-21) |

**Conclusão (2026-10-21):** o **fio NMD1** continua o contrato com o OS e o
**lag de produto fechou**: o OS consome o crate por **submodule pinado em
v1.4.5** (gitlink `16c0c29`), não mais por cópia/junction. Writer e leitor
partilham o mesmo `META_VERSION = 8`, e o pin é verificado por hash — ver
§"Pin do codec MDM1" abaixo.

### Re-sync MDM1 v8 (2026-10-19) — superado pelo submodule

> **Superado em 2026-10-21.** O backport manual abaixo foi **aposentado**: o
> submodule agora traz o crate 1.4.5 inteiro (não só o codec). Fica como
> registro de como o bug do decode foi encontrado.

O que mudou no vendored (`crates/neural-sgdb`, na época cópia **gitignored** no
OS — não era um commit do repositório do OS):

- `MemoryMeta` ganhou `scope_dims` (v7), `model_id` (v7) e `authority` (v8);
  `ScopeDims` foi portado como tipo próprio. `encode`/`decode` agora cobrem
  **v1–v8** com migração explícita (v1–v7 → `authority = 0`; v1–v6 → dims
  vazias, com o `scope` legado alimentando `user`).
- **Bug latente fechado**: o decode do `content_type` (v6) lia a string e não
  avançava `off` — inofensivo enquanto v6 era o último campo, fatal com o v7
  atrás dele (`trunc scopedim`). É a mesma disciplina "todo campo avança off"
  das migrações anteriores.
- Golden novo **`golden_mdm1_v8_bytes_from_standalone_roundtrip`**: 224 bytes
  emitidos pelo encoder 1.4.5, pinados no teste do vendored. Se o encode
  divergir um byte, o teste falha.

**Verificação executada:**

| Check | Resultado |
|-------|-----------|
| Suíte lib do vendored (cópia fora do workspace OS) | **245/245 pass** |
| Golden 224 B (encode vendored == encode standalone) | pass |
| **DB real**: meta escrita pelo standalone 1.4.5 (`FileStorage`, 176 B, version byte 8) lida pelo vendored | **pass** — `scope_dims=ana/planner/loja/s1`, `model=all-MiniLM-L6-v2-384`, `authority=255`, entities preservadas |

**Limitação de ambiente:** o workspace do OS pina
`nightly-2026-07-05-x86_64-pc-windows-msvc` e não há `link.exe` do MSVC nesta
máquina; os testes rodaram sobre uma cópia do vendored com o toolchain default
GNU. O código é o mesmo; o gate oficial do OS continua sendo `cargo test`
dentro do workspace, num ambiente com MSVC.

> **Nota MDM1 anterior (2026-10-16):** o v8 (`authority`) é aditivo no wire, mas
> um leitor v7 recusava a meta v8 (`decode` validava `1..=8`). O NMD1 sempre
> seguiu byte-idêntico; o que divergia era só o side-table da meta. Mapa
> versão→campo: `MIGRATIONS.md` §"MDM1 — evolução dos campos".

## Pin do codec MDM1 (submodule, desde 2026-10-21)

O OS consome este repo como **submodule** em `crates/neural-sgdb`:

| Artefacto (no OS) | Papel |
|---|---|
| `.gitmodules` | path + URL `https://github.com/msrovani/neural-sgdb.git` |
| gitlink (index, modo `160000`) | **commit pinado** — é a verdade |
| `tools/sgdb_pin.toml` | tag, commit, `crate_version`, `META_VERSION`, `sha256(src/memory_doc.rs)` |
| `tools/check_sgdb_pin.py` | gate: 7 checks; falha se gitlink/checkout/versão/codec divergirem |
| `.github/workflows/ci.yml` | `checkout` com `submodules: recursive` + o gate antes do `cargo check` |

**Por que assim:** antes o OS usava uma junction para `C:\DEV
eural-sgdb`
(`tools/sync-neural-sgdb.ps1`) que seguia a tip. O clone ficou em **1.1.13 /
`META_VERSION = 6`** enquanto o standalone chegou a **1.4.5 / v8**, e nada ficou
vermelho — o OS simplesmente recusava as metas novas. Além disso um clone limpo
do OS não tinha a dependência (path local, gitignored), logo o CI Linux não
compilava `k_ai`. O submodule resolve os dois: dependência reproduzível e
divergência **ruidosa**.

### Ritual de bump (explícito, em um commit)

```sh
cd <neural-os-core>
git -C crates/neural-sgdb fetch --tags
git -C crates/neural-sgdb checkout v1.4.6          # tag desejada
python3 tools/check_sgdb_pin.py --write-pin --tag v1.4.6
cargo check -p k_ai --target x86_64-unknown-none    # no_std continua compilando
git add crates/neural-sgdb tools/sgdb_pin.toml .gitmodules
```

O `--write-pin` recusa se o checkout não for exatamente `tag^{commit}`. O diff
mostra a mudança do gitlink e do hash no mesmo commit — não dá para "atualizar
sem querer".

### O que continua valendo

1. Manter o teste `golden_nmd1_bytes_match_neural_sgdb` verde no `k_ai`.
2. Não reintroduzir deps no core; não alterar encode NMD1/TKLV sem ADR + golden
   nos **dois** repos no mesmo release train.
3. **Nunca** editar `crates/neural-sgdb/` in-place: o gate acusa worktree suja.
   Correção no codec = commit aqui, release, e bump do pin.

## O que **não** é interop OS

- MCP stdio / Cursor / connectors — host-only.
- `commit_run` / MOM entities — convenção de agente; o OS consome Hits/NMD1.
- DB `.nsgdb/memory.db` partilhado entre projetos IDE ≠ sync CRDT com o kernel.

## Portabilidade multi-projeto (IDE)

| Padrão | OK? |
|--------|-----|
| Workspace `.cursor/mcp.json` com `DEFAULT_SCOPE=project/<este-repo>` | Sim |
| Global `~\.cursor\mcp.json` com `DEFAULT_SCOPE` de **outro** repo + mesmo `NEURAL_SGDB_DB` | **Anti-padrão** — null-scoping “esconde” o projeto aberto |
| Um DB por projeto **ou** scopes sempre explícitos nas tools | Sim |
| Dois writers no mesmo FileStorage | Não |
| Dois DBs / nós → `p2p` / `telepathy_two_db` | Sim |

Ver também: [`harness-prompts.md`](harness-prompts.md), [`MCP-RELOAD.md`](MCP-RELOAD.md).
