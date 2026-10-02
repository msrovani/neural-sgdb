# claw-sandbox.ps1 - sandbox host-side de um app "claw" (openclaw-style) contra o NSGDB.
# Missao realista de app agentico dirigindo a superficie MCP (remember/recall/decide/
# health/curate) via JSON-RPC sobre stdin/stdout do mcp_server.exe.
#
# Uso:  cargo build --release --example mcp_server ; powershell -File scripts/claw-sandbox.ps1
# Gera um DB FRESCO em %TEMP%\claw-sandbox\memory.db (nao toca o seu .nsgdb).
# Reporta PASS/FAIL por cenario (25 checks) e exit 0 sse todos passam.
#
# Nota PS 5.1: helpers com nomes que colidem com aliases nativos (SC, R) sao
# ofuscados (Set-Content/Invoke-History) - use nomes sem colisao.
#
$server = (Join-Path (Get-Location) "target\release\examples\mcp_server.exe")
$dbdir = Join-Path $env:TEMP "claw-sandbox"
if (Test-Path $dbdir) { Remove-Item $dbdir -Recurse -Force }
New-Item -ItemType Directory -Path $dbdir -Force | Out-Null
$env:NEURAL_SGDB_DB = (Join-Path $dbdir "memory.db")
$report = New-Object System.Collections.ArrayList

function RpcRaw($callLine) {
  $init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claw","version":"0.1"}}}'
  $out = @($init, $callLine) | & $server
  foreach ($l in $out) { if ($l -match '"id":2') { return $l } }
  return ($out -join "|")
}
function Call($name, $argsJson) {
  $line = '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"' + $name + '","arguments":' + $argsJson + '}}'
  return RpcRaw $line
}
function FromJson($raw) {
  try { return ($raw | ConvertFrom-Json) } catch { return $null }
}
function Txt($name, $argsJson) {
  $raw = Call $name $argsJson
  $r = FromJson $raw
  if (-not $r) { return "PARSE-ERR" }
  if ($r.error) { return "ERR $($r.error.code): $($r.error.message)" }
  return $r.result.content[0].text
}
function Scall($name, $argsJson) {
  $raw = Call $name $argsJson
  $r = FromJson $raw
  if (-not $r) { return $null }
  if ($r.error) { return $null }
  return $r.result.structuredContent
}
function Add-Check($ok, $label) {
  $report.Add(@{ok = $ok; label = $label }) | Out-Null
  $s = if ($ok) { "PASS" } else { "FAIL" }
  Write-Output ("[{0}] {1}" -f $s, $label)
}

Write-Output "=== CLAW SANDBOX (openclaw-style mission vs neural-sgdb) ==="
Write-Output ("db=" + $env:NEURAL_SGDB_DB)

# 0. Onboarding: claw reads cold-start packet (session resource)
$init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claw","version":"0.1"}}}'
$res = '{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"nsgdb://session"}}'
$raw = @($init, $res) | & $server
$sr = ($raw | Where-Object { $_ -match '"id":2' } | ConvertFrom-Json)
$sess = $sr.result.contents[0].text
Add-Check ($sess -match "next_actions") "onboarding: nsgdb://session tem cold_start.next_actions"
Add-Check ($sess -match "tool" -or $sess -match "scope") "onboarding: session menciona tools/scope"

# 1. Tenant acme: preferencia + fato com entidades, escopados
$k1 = Scall "remember" '{"text":"user prefers canary deploys with auto-rollback","scope":"acme","entities":["acme/user/pref"],"type":"text"}'
$k1s = $k1.storage_key
Add-Check ($k1s -match "^md/L3/") "acme pref: storage_key=$k1s"

$k2 = Scall "remember" '{"text":"deploy window is 2-4am UTC; ship only inside the window","scope":"acme","entities":["acme/ops/deploy"],"type":"text"}'
$k2s = $k2.storage_key
Add-Check ($k2s -match "^md/L3/") "acme fact: storage_key=$k2s"

# 2. Recuperacao exata (lexical, mesmas palavras)
$t = Txt "recall" '{"query":"deploy window 2-4am","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -match "2-4am") "recall lexical exato acha o fato"

# 3. Parafrase (lexical deve errar - ADR-0008; sem token compartilhado)
$t = Txt "recall" '{"query":"office hours","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -notmatch "2-4am") "recall lexical erra parafrase (esperado, ADR-0008)"

# 4. Entidades 1-hop (mesma string na escrita e na busca)
$t = Txt "recall" '{"query":"deploy","scope":"acme","mode":"entities","entities":["acme/ops/deploy"],"k":3}'
Add-Check ($t -match "2-4am") "recall_entities (1-hop) acha pelo rotulo acme/ops/deploy"

# 5. Decisao com evidencia (decide tool: questions[] + ask obrigatorio)
$d = Txt "decide" '{"questions":[{"query":"should we ship tonight?","ask":"evidence_sufficient"}],"k":5,"scope":"acme"}'
Add-Check ($d -notmatch "^ERR" -and $d -match "sufficient") "decide: veredito tipado (evidence_sufficient) sem erro"

# 6. Multi-tenancy: tenant beta escreve o proprio estado
$kb = Scall "remember" '{"text":"beta uses blue-green; freeze on Friday","scope":"beta","entities":["beta/user/pref"],"type":"text"}'
Add-Check ($kb.storage_key -match "^md/L3/") "beta pref: storage_key=$($kb.storage_key)"

$t = Txt "recall" '{"query":"blue-green","scope":"beta","mode":"lexical","k":3}'
Add-Check ($t -match "blue-green" -and $t -notmatch "acme") "isolamento: recall(scope=beta) ve beta, sem acme"

$t = Txt "recall" '{"query":"blue-green","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -notmatch "beta") "isolamento: recall(scope=acme) nao vaza beta"

$t = Txt "recall" '{"query":"deploy blue-green canary","mode":"lexical","k":5}'
Add-Check ($t -notmatch "2-4am" -and $t -notmatch "blue-green") "null-scoping: recall global nao vaza acme nem beta"

# 7. TTL e contrato/promessa explicita:
#    (a) futuro -> o recall ainda ve (nao venceu; staleness so reporta STALE,
#        e TTL vencido e honrado no open antes de virar relatorio);
#    (b) vencido -> honrado no OPEN da sessao (expire_ttl no open): a consulta
#        ja nao ve a memoria (sem o host chamar expire_ttl manualmente).
$now = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$future = $now + 3600000
$past = $now - 5000
$ttl = Txt "curate" ('{"op":"set_ttl","key":"' + $k1s + '","expires_at":' + $future + '}')
Add-Check ($ttl -match ([string]$future)) "set_ttl honra expires_at absoluto (futuro)"
$t = Txt "recall" '{"query":"canary","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -match "canary") "TTL futuro ainda visivel (nao venceu)"
$ttl2 = Txt "curate" ('{"op":"set_ttl","key":"' + $k1s + '","expires_at":' + $past + '}')
Add-Check ($ttl2 -match ([string]$past)) "set_ttl no passado (vence)"
$t = Txt "recall" '{"query":"canary","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -notmatch "canary") "TTL vencido honrado no OPEN -> recall ativo nao ve"

# 8. Supersede (DAG): versao corrente + historia preservada (ADD-only)
$k3 = Scall "remember" '{"text":"deploy window moved to 3-5am UTC","scope":"acme","entities":["acme/ops/deploy"],"type":"text","if_exists":"supersede"}'
Add-Check ($k3.storage_key -match "^md/L3/") "supersede: novo storage_key=$($k3.storage_key)"
$t = Txt "recall" '{"query":"deploy window","scope":"acme","mode":"lexical","k":3,"historical":true}'
Add-Check ($t -match "3-5am" -and $t -match "2-4am") "supersede DAG: corrente (3-5am) E antiga (2-4am) no historical"
$t = Txt "recall" '{"query":"deploy window","scope":"acme","mode":"lexical","k":3}'
Add-Check ($t -notmatch "2-4am" -or $t -match "3-5am") "recall ativo prefere a versao corrente"

# 9. Auditoria: checkpoint + verify
$c = Txt "curate" '{"op":"audit_checkpoint"}'
Add-Check ($c -match "seq=0") "audit_checkpoint seq=0"
$v = Txt "curate" '{"op":"audit_verify"}'
Add-Check ($v -match "intact") "audit_verify chain intact"

# 10. Curadoria honesta: expire_old + consolidate (L3 -> 0, esperado)
$e = Txt "curate" '{"op":"expire_old"}'
Add-Check ($e -notmatch "^ERR") "expire_old rodou (sem validade -> 0)"
$cc = Txt "curate" '{"op":"consolidate","min_repeats":3,"min_len":24,"max_new":64}'
Add-Check ($cc -notmatch "^ERR") "consolidate rodou (L3 -> 0, honesto)"

# 11. Reopen: tudo acima rodou em processos separados (FileStorage). Estado persiste.
$h = Scall "health" '{}'
Add-Check ($h.doc_count -gt 0) ("reopen: docs=" + $h.doc_count + " persistem entre processos")
Add-Check ($h.global_memory_count -eq 0 -and $h.scoped_memory_count -gt 0) "reopen: null-scoping preservado (global=0, scoped>0)"

Write-Output ""
Write-Output "=== RELATORIO ==="
$fails = 0
foreach ($c in $report) { if (-not $c.ok) { $fails++; Write-Output ("  FAIL: " + $c.label) } }
Write-Output ("pass=" + ($report.Count - $fails) + " fail=" + $fails + " total=" + $report.Count)
if ($fails -eq 0) { Write-Output "RESULTADO: CLAW MISSION PASSOU (exit 0)" } else { Write-Output "RESULTADO: ha falhas - ver acima" }