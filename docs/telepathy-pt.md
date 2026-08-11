# Telepatia — sincronização de memória p2p no neural-sgdb

> **Memória, não pacotes.** Duas instâncias de `Sgdb` trocam suas memórias com
> versionamento, causalidade e preservação de conflitos — sem servidor central.
> Rode a demo: `cargo run --release --example p2p_telepathy --features p2p`.

Este documento explica (a) o CRDT que move a troca, (b) o fluxo da telepatia
entre duas instâncias, (c) o custo honesto do modelo (consistência eventual,
sem ordenação global, conflitos preservados), e (d) como uma IA na raiz do
processo arbitra os conflitos preservados.

---

## 1. O CRDT — o que roda de fato

`CrdtMemorySync` é um **CRDT de contador/versão com preservação de conflito**.
Cada nó mantém um estado local mínimo:

| Campo | Papel |
|---|---|
| `local_version` | contador monotônico das **próprias** escritas |
| `own_writes` | nº de escritas locais independentes — a **base** da detecção de concorrência (sem ele, um sucessor causal do mesmo peer viraria conflito eterno; corrigido na revisão v0.3) |
| `node_versions` | o que eu sei sobre cada peer: `(node_id, version)` |
| `conflicts` | versões concorrentes **preservadas** — nunca descartadas por LWW cego |
| `pending` (delta, #10) | versões locais ainda não entregues aos peers |

O merge roda por veredicto explícito (`apply_remote_version`):

| Veredito | Significado |
|---|---|
| `SelfPacket` | eco do meu próprio broadcast → ignora |
| `Stale` / `Duplicate` | `v ≤` conhecido → ignora, **sem regressão** |
| `Applied` | versão nova, sem estado local conflitante → adota |
| `Conflict` | versão concorrente (escrevi algo independente) → guarda em **ambos** `node_versions` e `conflicts` |

Na demo da telepatia você vê isso ao vivo: A escreve `m1` e B escreve `m2`
antes de se conhecerem → ambos os lados logam `CONFLITO preservado
(concorrente)`. Nenhuma escrita é perdida.

Desde a v0.5 o sync é **baseado em delta** (#10): `record_change` acumula
`pending` deltas e `sync` envia só o que o peer ainda não viu (`send_delta`,
cujo default na trait cai para `send_crdt`) — payload ∝ mudanças não-vistas,
não a história completa.

---

## 2. O fluxo da telepatia — duas instâncias convergem

Cada instância = `Sgdb` + `CrdtMemorySync`. A troca tem duas fases:

1. **Sync de versões (o gatilho causal).** `sync()` troca versões por um
   `Transport` (na demo, um pipe em memória; no mundo real, `UdpTransport`,
   TLS, serial). Cada nó aprende o `local_version` do peer.
2. **Pull por diff (o payload).** Quando um nó aprende que o peer avançou,
   replica os docs que faltam: `Sgdb::get(layer, key)` → `Sgdb::put(doc)` —
   idempotente, chaveado por storage key. `Sgdb::put` é a primitiva pública de
   restore/import que re-indexa qualquer `MemoryDoc` (ART/BQ/lexical).

Fluxo e saída da demo:

```
[A] lembra m1  (versão A = 1)
[B] lembra m2  (versão B = 1)
CRDT sync: node=2 v=1 CONFLITO preservado (concorrente)
CRDT sync: node=1 v=1 CONFLITO preservado (concorrente)
[↔] ronda 1: A→B 2 doc(s), B→A 2 doc(s)
[↔] ronda 2: A→B 0 doc(s)          ← idempotente, já convergidos
[↔] ronda 3: B→A 2 doc(s)          ← B responde m3, volta para A
[✓] A conhece 6 docs, B conhece 6 docs
[B] recall da memória de A: ["eu sou a instancia A..."]  ← telepatia semântica
```

Duas instâncias convergem **sem servidor central**. O resultado da arbitragem
(§4) é ele próprio uma escrita causal nova — então as resoluções se propagam a
todos os peers pelo mesmo mecanismo.

---

## 3. O custo honesto

### 3.1 Consistência eventual — "divergem temporariamente"

Modelo tradicional: leia o servidor → todo mundo vê o mesmo estado a qualquer
instante. CRDT: cada nó é dono do estado local e só troca quando os nós se
encontram (`sync` é rate-limited por `SYNC_INTERVAL` e exige conectividade).

Antes da ronda 1 da demo, A só conhece `{m1}` e B só conhece `{m2}` — uma query
em A responde diferente de uma em B. Essa é a **janela de divergência** entre a
escrita e o sync.

A convergência é **garantida** (o merge é comutativo, associativo e idempotente;
`apply_remote_version` nunca regride — contadores são monotônicos), só não é
**datada**: não há "quando", apenas "se houver sync suficiente". O pior caso é
divergir por mais tempo; nunca dados perdidos ou ressuscitados.

### 3.2 Sem ordenação global

Um banco central serializa as escritas → existe uma ordem total (timestamp do
servidor). Um CRDT não tem sequência global: cada nó conta com o seu próprio
`local_version`. Duas escritas em nós diferentes **não têm** um antes/depois
global.

O que o CRDT *dá* é **ordem causal**: uma resposta escrita após receber a
memória de outro nó é causalmente posterior a ela (sabível localmente),
enquanto duas escritas independentes são **concorrentes** — incomparáveis.

Consequência: "qual é a última?" não tem resposta global. Métodos que precisam
de tempo real recebem `now: u64` **do caller** (relógio de parede), não do CRDT
— o CRDT sabe causalidade, não tempo. É exatamente a lacuna que a camada
superior preenche.

### 3.3 Conflitos preservados, não resolvidos

Se A escreve `"user prefere dark mode"` e B escreve `"user prefere light
mode"` concorrentemente, o CRDT:
1. marca `Conflict`;
2. guarda **ambas** em `conflicts` e `node_versions`;
3. não descarta nada.

LWW cego (maior versão vence) exige um "última" global — que **não existe**
para versões concorrentes (§3.2). E mesmo que existisse, a resolução é
**semântica**: qual preferência é verdadeira exige entender o conteúdo, não
ordená-lo. O CRDT não deve *perder* uma versão; decidir é do caller.

A camada superior resolve **na leitura**, com as políticas que o crate entrega:
- **`recall_weighted`** — `score = w_sem·dist + w_rec·recência + w_imp·
  importância` (recência do `/ts/<hex>` de parede na key, importância da
  camada). A versão mais nova/importante outranqueia **no uso**, não na gravação.
- **`recall_at` + janela de validade** (`sys/validity/`) — uma versão
  invalidada some do recall enquanto a história permanece.
- **`conflicts` exposto** (multi-value) — a aplicação (ou um LLM, ou um
  operador) pode inspecionar as duas versões e decidir, explícita e auditavelmente.

A diferença filosófica: o modelo tradicional resolve na **gravação** (o servidor
escolhe e destrói a perdedora — perda silenciosa); o CRDT resolve na **leitura**
(tudo é preservado; a política do momento decide). Você troca "forte por
padrão" por "nunca perde + decisão consciente".

---

## 4. Arbitragem — uma IA na raiz do processo

A IA na raiz do processo (no OS pai, ela roda desde o boot) é exatamente a
"camada superior" para a qual o CRDT adia a resolução. Ela arbitra assim:

### 4.1 Colheita — o CRDT garante informação completa

A raiz enumera os conflitos (`conflicts` é público) e lê **as duas versões
inteiras**: `layer`, `clock` (causalidade), payload, e a recência real
(`/ts/<hex>` na key). Diferente do modelo tradicional — onde o servidor já
destruiu a perdedora e a arbitragem roda sobre dados truncados — aqui nada se
perdeu: a análise é sobre as duas versões mais o histórico causal.

### 4.2 Contexto — arbitrar com a memória episódica

Para decidir `"dark mode"` vs `"light mode"`, a raiz segue a cadeia causal até
as memórias L2/L3 associadas: "quando o usuário reclamou de brilho pela última
vez?", "o que o assistente respondeu?". Isso é `scan_prefix` + `get` guiados
pelo relógio vetorial.

### 4.3 Sinais — pesar evidência ordenável

- **Recência** (`recall_weighted`, `w_rec`): `ts` de parede mais novo.
- **Importância** (`w_imp` por camada): um fato L4 semântico pesa mais que uma
  nota L2 episódica.
- **Consistência com comportamento**: a versão que bate com as evidências
  episódicas.

### 4.4 Decisão — quatro políticas de arbitragem (da barata à cara)

| Política | Mecanismo no repo | Quando |
|---|---|---|
| **Recency-first** (determinística, sem IA) | `recall_weighted` com `w_rec` alto — resolve **na leitura** | casos corriqueiros, barato |
| **Validade** | `invalidate(key, now)` / `recall_at` — a perdedora **some da visão**, história preservada | obsoleta mas não deletável |
| **Fusão semântica** | raiz lê as duas → escreve doc novo que une → `supersede(old, new)` + `set_state(Superseded)` | conflito de conteúdo real ("dark de dia, light de noite") |
| **Escalada** | mantém em `conflicts`, loga, expõe para humano/agente | ambiguidade que exige o usuário |

### 4.5 Materializa e propaga

A resolução não é local: a raiz grava o resultado e chama `record_change()` →
o novo doc (ou a invalidação) é uma **escrita causal nova** → a próxima ronda
de telepatia espalha o veredito para todos os peers. A arbitragem converge na
rede, e a cadeia fica auditável (quem arbitrou, quando, com base em quê).

### 4.6 Exemplo concreto

A escreve `dark mode` (v1); B escreve `light mode` (v1) → `Conflict`.
1. A raiz lê os dois mais a cadeia causal L2 → descobre que `dark mode` tem
   `ts` mais novo **e** o usuário depois disse "diminui o brilho, meus olhos
   estão cansados".
2. Veredito: `light mode` está obsoleto → `invalidate("md/L4/pref", ts_dark)`
   (recência + evidência).
3. O conflito é marcado resolvido; `dark mode` continua como único visível em
   `recall_at`.
4. `sys/validity/` propaga via telepatia → B também para de oferecer `light
   mode`.

### 4.7 Por que é mais forte que o modelo tradicional

No modelo central, a "arbitragem" é um trigger/checkpoint no servidor — uma
política, uma hora, sobre dados já truncados. Aqui a raiz arbitra **a qualquer
momento, com as duas versões, a cadeia causal completa e o relógio de parede**,
e o veredito **vira estado CRDT que converge**. É a diferença entre um banco
que apaga e um cérebro que decide com o histórico inteiro na mesa.

---

## 5. Como rodar

```bash
# duas instâncias Sgdb trocam memórias (CRDT version sync + diff-pull)
cargo run --release --example p2p_telepathy --features p2p

# benchmarks / stress / MCP server
cargo run --release --example bench
cargo run --release --example stress
cargo run --release --example mcp_server
```

Implementação: `src/crdt.rs`, `examples/p2p_telepathy.rs`, `Sgdb::put` em
`src/sgdb.rs`. Relacionados: `recall_weighted`, `recall_at`, `sys/validity/`
(§3.3/§4), sync por delta (#10).

Versão em inglês: [`telepathy.md`](telepathy.md).
