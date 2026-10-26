#!/usr/bin/env python3
"""Probe runtime: escrita operacional (author=system) em L4.

Verifica pela INTERFACE REAL (MCP stdio) tres asseracoes do estudo
`docs/layers-usage-analysis.md`:

- A3: `scope`/`entities` passados no request sao descartados em silencio.
- A4: com `embedding=`, o texto e perdido (payload vira f32, sem companion)
      -> a busca lexical pelo texto NAO acha a memoria.
- A8: nenhuma view do `health` conta por camada.

Resultado (2026-10-16, crate 1.4.5): A3, A4 e A8 CONFIRMADOS em runtime — o
response ecoa `scope:""`/`entities:[]`, o recall lexical pelo texto devolve `[]`
com `companion_key:""`, o recall SEMANTICO pelo mesmo vetor acha o doc (logo o
que se perdeu foi o texto, nao a memoria), e o `health` nao tem campo de camada.

Uso: python3 scripts/probe-layers-mcp.py [caminho/do/mcp_server]
Nao toca o DB real: usa NEURAL_SGDB_DB num ficheiro temporario.
"""
import json
import os
import subprocess
import sys
import tempfile

BIN = os.path.abspath(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/examples/mcp_server.exe"
)
SENTINEL = "SENTINELA_L4_TEXTO_9f2c"
KEY = "probe/l4/sentinela"
SCOPE = "probe/scope"

db = os.path.join(tempfile.gettempdir(), "nsgdb-probe-layers.db")
for suffix in ("", ".tk", ".tickv"):
    try:
        os.remove(db + suffix)
    except OSError:
        pass

env = dict(os.environ)
env["NEURAL_SGDB_DB"] = db
env.pop("NEURAL_SGDB_EMBEDDER", None)  # sem embedder de host: vetor vai no payload

proc = subprocess.Popen(
    [BIN],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    env=env,
    text=True,
    encoding="utf-8",
    errors="replace",
)


def send(req):
    proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    while True:
        line = proc.stdout.readline()
        if not line:
            raise SystemExit("servidor fechou stdout antes da resposta")
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("id") == req["id"]:
            return msg


def call(i, name, **args):
    return send({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                 "params": {"name": name, "arguments": args}})


send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                 "clientInfo": {"name": "probe", "version": "0"}}})

# 1) escrita operacional L4 com texto, embedding, scope e entities
w = call(2, "remember", author="system", layer="L4", key=KEY, text=SENTINEL,
         embedding=[0.1, 0.2, 0.3, 0.4],
         scope=SCOPE, entities=["mom/probe"], type="text")
sc = w.get("result", {}).get("structuredContent", {})
print("== remember(author=system, layer=L4) ==")
print("  storage_key   :", sc.get("storage_key"))
print("  companion_key :", repr(sc.get("companion_key")))
print("  scope ecoado  :", repr(sc.get("scope")))
print("  entities ecoadas:", sc.get("entities"))
print("  index_key/text:", "text perdido se payload==", "f32 bytes")
print("  (A3) scope/entities ecoados vazios => descartados" if not sc.get("entities") and not sc.get("scope") else "  (A3) params honrados")

# 2) a busca lexical pelo TEXTO acha a memoria?
for label, args in (("sem scope", {}), ("com scope", {"scope": SCOPE})):
    r = call(3 if label == "sem scope" else 4, "recall",
             query=SENTINEL, mode="lexical", format="json", k=5, **args)
    res = r.get("result", {})
    text = res.get("content", [{}])[0].get("text", "")
    hits = 0
    try:
        hits = len(json.loads(text.split("\n", 1)[-1]) if text.startswith("[") or text.startswith("{") else json.loads(text))
    except (json.JSONDecodeError, ValueError, IndexError):
        for key in ("\"key\"",):
            hits = text.count(key)
    print(f"== recall lexical {label} ==")
    print("  hits:", hits if isinstance(hits, int) else hits)
    print("  raw :", text[:200].replace("\n", " "))

# 2b) recall SEMANTICO pelo mesmo vetor: o doc esta la (so o TEXTO se perdeu)
s = call(7, "recall", query="probe", mode="semantic", k=5,
         embedding=[0.1, 0.2, 0.3, 0.4], format="json")
stxt = s.get("result", {}).get("content", [{}])[0].get("text", "")
print("== recall semantic (mesmo vetor) ==")
print("  raw :", stxt[:220].replace("\n", " "))
print("  => doc alcançavel, TEXTO vazio" if KEY in stxt else "  => doc NAO encontrado")

# 3) explain: qual camada o doc realmente tem?
e = call(5, "curate", op="explain", key=f"md/L4/{KEY}")
etxt = e.get("result", {}).get("content", [{}])[0].get("text", "")
print("== curate op=explain ==")
print("  ", etxt[:240].replace("\n", " "))

# 4) health: existe contagem por camada?
h = call(6, "health")
hc = h.get("result", {}).get("structuredContent", {})
layer_fields = [k for k in hc if "layer" in k.lower()]
print("== health ==")
print("  campos com 'layer':", layer_fields or "(nenhum)")
print("  doc_count:", hc.get("doc_count"), "| ram_len:", hc.get("ram_len"))
print("  mcp_tool_count:", hc.get("mcp_tool_count"))

proc.stdin.close()
proc.terminate()
print("\nDB temporario:", db)
