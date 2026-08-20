# Contributing — neural-sgdb

Obrigado por contribuir. Leia `AGENTS.md`, `codemap.md` e `docs/api.md` antes de
editar código.

## Setup

```bash
git clone https://github.com/msrovani/neural-sgdb.git
cd neural-sgdb
cargo build --release --example mcp_server --target-dir target/mcp-release
cargo test
cargo check --no-default-features --target x86_64-unknown-none
```

## Identidade Git (obrigatório para commit)

Configure **antes** do primeiro commit nesta máquina:

```bash
git config --global user.name "Seu Nome"
git config --global user.email "seu@email.com"
```

Ou apenas neste repositório (sem `--global`).

## Gates locais (espelham CI)

```bash
cargo test
cargo test --features p2p
cargo test --no-default-features
cargo clippy --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo check --no-default-features --target x86_64-unknown-none
bash scripts/mcp-smoke.sh
# Windows: powershell -File scripts/mcp-install.ps1; powershell -File scripts/mcp-smoke.ps1
```

## Regras do crate

1. Zero deps em `[dependencies]` — só `alloc`/`std`
2. NMD1/TKLV byte-identical com neural-os-core — golden tests pinados
3. Side-tables para metadata nova (MDM1 vN) — nunca reinterpretar bytes antigos
4. Cada bugfix/regressão com teste quando fizer sentido
5. `cargo fmt` não é gate — o repo não é rustfmt-clean

## Commits

Conventional Commits curtos, foco no **porquê**:

```
fix(mcp): surface era_report hint on dimension mismatch

docs: align architecture docs to v1.1.6
```

## MCP / agentes

Instalação e troubleshooting: [`docs/MCP.md`](docs/MCP.md).
