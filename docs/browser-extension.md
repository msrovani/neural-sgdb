# Extensão de navegador — ESTACIONADA

> **Não é o produto.** NSGDB é memória **agêntica** (MCP `remember`/`recall`/
> `health`/`curate`, doutrina, protocolos em `examples/agent_protocol.rs`).
> Esta pasta/docs descreve um stub de 1.1.13. Auto-captura de abas está
> **comentada**. Store **não** é gate de release (`VERSIONING.md` passo 6).
>
> Não instalar, não republicar, não tratar como feature. Reabrir só com ADR.

O texto abaixo é arquivo histórico (o que se pretendia). Não reflete o que
o crate entrega hoje.

---

# (arquivo) Extensão de navegador — nsgdb local-first

> Memória do uso do navegador, 100% local, sem nuvem, sem API key.
> Mesmo prático do `mem0`/`Supermemory` (1 clique), mas verbatim + IndexedDB.
> Complementa `docs/adr/0008-local-embedder.md` Other uses e `crates/nsgdb-wasm`.

## O que faz (pretendido — não ligado)

A extensão *pretendia* capturar o uso do navegador como memória verbatim
(`md/L3/` léxico). O `background.js` atual tem os listeners **comentados**.

Diferença para `mem0`: nsgdb guarda **verbatim** local — léxico acha sem
embedder. Isso continua verdadeiro **no core/MCP**, não neste stub.

## Instalação

Não. Track estacionado.

## Permissões

`history` / `tabs` / `scripting` / `<all_urls>` eram o viés de hoarding.
Desligadas no código (listeners comentados); o `manifest.json` ainda lista
as permissões do stub e **não deve ir à Store**.
