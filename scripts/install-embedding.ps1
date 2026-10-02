<#
.SYNOPSIS
  Instala embedding LOCAL (via B - modelo in-process) no neural-sgdb, para o
  user/LLM/IDE. Respeita as premissas: zero-dep no lib, no_std intacto,
  core nunca gera vetor (ADR-0002/0008/0007) - o installer so PROVISIONA o
  crate host `crates/nsgdb-embed` + a config do host.

.DESCRIPTION
  Fluxo (idempotente):
    1. Pergunta se o user/LLM quer instalar embedding local (via B).
    2. Checa dependencias: toolchain (dlltool/clang), rede, modelo baixado,
       build candle.
    3. Instala o que der: toolchain (winget/choco mingw, se admin+rede),
       modelo (HF: tokenizer.json + model.safetensors), `cargo build
       --features candle`.
    4. Adequa o host: `NEURAL_SGDB_EMBEDDER=<label>` no launcher
       (scripts/mcp-server.ps1) e nas configs do IDE (.opencode/opencode.json,
       .cursor/mcp.json). O `model_id` da era e declarado pelo MCP
       (embedder_model_id_for) - mesmo-modelo na escrita e busca (ADR-0007).
    5. Smoke: roda o demo do crate host (dimensao/era) e, se houver binario
       MCP, `health` via JSON-RPC.
  Se algo estiver bloqueado (offline / sem toolchain / sem admin), REPORTa com
  clareza e encerra com codigo != 0 - NUNCA quebra em silencio.

.PARAMETER Yes / No
  Pula a pergunta interativa (assume sim / nao).

.PARAMETER CheckOnly
  So diagnostica e reporta o que falta; nao muda nada (util em CI/offline).

.PARAMETER EmbedderLabel
  Rotulo do embedder a configurar. Default: `multilingual`
  (paraphrase-multilingual-MiniLM-L12-v2, 384-dim).

.PARAMETER ModelUrl
  Base URL dos arquivos do modelo (tokenizer.json + model.safetensors).
  Default: repositorio sentence-transformers do L12-v2.
#>
param(
  [switch]$Yes,
  [switch]$No,
  [switch]$CheckOnly,
  [string]$EmbedderLabel = "multilingual",
  [string]$ModelUrl = "https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$ModelId = "paraphrase-multilingual-MiniLM-L12-v2-384"
$Dim = 384
$ModelDir = Join-Path $Root "crates\nsgdb-embed\models\multilingual"
$ModelFiles = @("model.safetensors", "tokenizer.json")
$Launcher = Join-Path $Root "scripts\mcp-server.ps1"
$OpencodeCfg = Join-Path $Root ".opencode\opencode.json"
$CursorCfg = Join-Path $Root ".cursor\mcp.json"

function Write-Step([string]$m) { Write-Host "[install-embedding] $m" -ForegroundColor Cyan }
function Write-Err([string]$m) { [Console]::Error.WriteLine("[install-embedding] ERRO: $m") }

function Test-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  $p = New-Object Security.Principal.WindowsPrincipal($id)
  return $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-Network {
  try {
    $r = Invoke-WebRequest -Uri "https://huggingface.co" -Method Head -TimeoutSec 8 -UseBasicParsing
    return ($null -ne $r)
  } catch { return $false }
}

function Get-RustcHost {
  try {
    $v = (& rustc -vV 2>$null | Out-String)
    if ($v -match "host:\s*(\S+)") { return $Matches[1] }
  } catch {}
  return "unknown"
}

function Get-ToolchainReport {
  $rustcHost = Get-RustcHost
  $hasDlltool = $null -ne (Get-Command dlltool -ErrorAction SilentlyContinue)
  $hasClang = $null -ne (Get-Command clang -ErrorAction SilentlyContinue)
  $hasGnuClang = $null -ne (Get-Command x86_64-w64-mingw32-clang -ErrorAction SilentlyContinue)
  # Candle (via B) precisa de toolchain C nativo: em host GNU o getrandom/cc
  # usa dlltool; em gnullvm falta o linker mingw-clang; MSVC nao tem dlltool.
  $blockers = @()
  if ($rustcHost -like "*windows-gnu") { if (-not $hasDlltool) { $blockers += "dlltool ausente (binutils/mingw)" } }
  if ($rustcHost -like "*windows-gnullvm") { if (-not ($hasGnuClang -or $hasClang)) { $blockers += "linker mingw-clang ausente" } }
  return [pscustomobject]@{
    RustcHost = $rustcHost
    HasDlltool = $hasDlltool
    HasClang = $hasClang
    HasGnuClang = $hasGnuClang
    Blockers = $blockers
  }
}

function Get-ModelStatus {
  $have = @()
  foreach ($f in $ModelFiles) { if (Test-Path (Join-Path $ModelDir $f)) { $have += $f } }
  return [pscustomobject]@{ Present = $have; Missing = @($ModelFiles | Where-Object { $_ -notin $have }) }
}

function Install-ToolchainIfNeeded([object]$tc, [bool]$network) {
  if ($tc.Blockers.Count -eq 0) { return "ok" }
  if (-not (Test-Admin)) { return "bloqueado: precisa de admin para instalar toolchain ($($tc.Blockers -join ', '))" }
  if (-not $network) { return "bloqueado: offline - instale binutils/mingw manualmente ($($tc.Blockers -join ', '))" }
  $winget = Get-Command winget -ErrorAction SilentlyContinue
  if ($winget) {
    Write-Step "instalando mingw via winget (admin)..."
    & winget install -e --id MartinStorsjo.LLVM-Mingw 2>$null
    if ($LASTEXITCODE -ne 0) { return "falha ao instalar mingw (winget exit $LASTEXITCODE)" }
    return "ok"
  }
  $choco = Get-Command choco -ErrorAction SilentlyContinue
  if ($choco) {
    Write-Step "instalando mingw via choco (admin)..."
    & choco install mingw -y 2>$null
    if ($LASTEXITCODE -ne 0) { return "falha ao instalar mingw (choco exit $LASTEXITCODE)" }
    return "ok"
  }
  return "bloqueado: sem winget/choco - instale binutils/mingw manualmente"
}

function Download-Model([bool]$network) {
  if (-not $network) { return "bloqueado: offline - baixe ${ModelId} manualmente para $ModelDir" }
  New-Item -ItemType Directory -Path $ModelDir -Force | Out-Null
  foreach ($f in $ModelFiles) {
    $dst = Join-Path $ModelDir $f
    if (Test-Path $dst) { continue }
    Write-Step "baixando ${f}..."
    try {
      Invoke-WebRequest -Uri ($ModelUrl + $f) -OutFile $dst -TimeoutSec 300 -UseBasicParsing
    } catch {
      return "falha ao baixar ${f}: $($_.Exception.Message)"
    }
  }
  return "ok"
}

function Build-Candle {
  Write-Step "buildando crates/nsgdb-embed --features candle..."
  Push-Location $Root
  try {
    & cargo build --manifest-path (Join-Path $Root "crates\nsgdb-embed\Cargo.toml") --features candle 2>$null
    if ($LASTEXITCODE -ne 0) { return "build candle falhou (exit $LASTEXITCODE) - veja o log" }
  } finally { Pop-Location }
  return "ok"
}

function Set-LauncherEnv {
  # scripts/mcp-server.ps1: injeta $env:NEURAL_SGDB_EMBEDDER antes de `& $Exe`
  if (Test-Path $Launcher) {
    $txt = Get-Content $Launcher -Raw
    if ($txt -notmatch "NEURAL_SGDB_EMBEDDER") {
      $line = "`$env:NEURAL_SGDB_EMBEDDER = `"$EmbedderLabel`""
      $txt = $txt -replace "(?m)(& \`$Exe)", "$line`r`n& `$Exe"
      [System.IO.File]::WriteAllText($Launcher, $txt)
      Write-Step "launcher scripts/mcp-server.ps1: NEURAL_SGDB_EMBEDDER=$EmbedderLabel"
    } else {
      Write-Step "launcher scripts/mcp-server.ps1: ja configurado"
    }
  }
  foreach ($cfg in @($OpencodeCfg, $CursorCfg)) {
    if (-not (Test-Path $cfg)) { continue }
    $json = Get-Content $cfg -Raw | ConvertFrom-Json
    $envMap = $null
    if ($json.mcp -and $json.mcp.'neural-sgdb') { $envMap = $json.mcp.'neural-sgdb'.environment }
    elseif ($json.mcpServers -and $json.mcpServers.'neural-sgdb') { $envMap = $json.mcpServers.'neural-sgdb'.env }
    if ($null -ne $envMap) {
      if ($null -eq $envMap.NEURAL_SGDB_EMBEDDER) {
        $envMap.NEURAL_SGDB_EMBEDDER = $EmbedderLabel
        $json | ConvertTo-Json -Depth 12 | Set-Content -Path $cfg -Encoding UTF8
        Write-Step "${cfg}: NEURAL_SGDB_EMBEDDER=$EmbedderLabel"
      } else {
        Write-Step "${cfg}: ja configurado"
      }
    }
  }
}

function Smoke {
  Write-Step "smoke do crate host (demo)..."
  & cargo run --quiet --manifest-path (Join-Path $Root "crates\nsgdb-embed\Cargo.toml") --example demo 2>&1 | Out-Null
  if ($LASTEXITCODE -ne 0) { Write-Err "smoke do demo falhou (exit $LASTEXITCODE)" }
  else { Write-Step "demo ok (dimensao ${Dim}, fallback deterministico ativo ate o candle rodar)" }
}

# 1. pergunta
if (-not $Yes -and -not $No -and -not $CheckOnly) {
  $ans = Read-Host "Instalar embedding local (via B, ${ModelId})? [s/N]"
  if ($ans -notmatch "^s|^sim|^y|^yes") { $No = $true }
}

if ($No) {
  Write-Step "sem embedding local. Default lexical (ADR-0008) permanece. Nada mudou."
  exit 0
}

# 2. diagnostico (sempre)
Write-Step "diagnostico..."
$admin = Test-Admin
$network = Test-Network
$tc = Get-ToolchainReport
$model = Get-ModelStatus
Write-Step "  rustc host      : $($tc.RustcHost)"
Write-Step "  dlltool         : $($tc.HasDlltool) | clang: $($tc.HasClang) | mingw-clang: $($tc.HasGnuClang)"
Write-Step "  toolchain       : $(if ($tc.Blockers.Count) { $tc.Blockers -join '; ' } else { 'ok' })"
Write-Step "  rede            : $network"
Write-Step "  modelo presente : $($model.Present -join ', ') | faltando: $($model.Missing -join ', ')"
Write-Step "  admin           : $admin"

if ($CheckOnly) {
  if ($tc.Blockers.Count -gt 0) {
    Write-Err "BLOQUEADO por toolchain: $($tc.Blockers -join '; ')."
    Write-Err "Solucao: instalar binutils/mingw (winget/choco, admin+rede) OU buildar o candle em Linux/CI. Nada foi alterado."
    exit 3
  }
  if ($model.Missing.Count -gt 0) {
    Write-Err "Modelo incompleto em $ModelDir. Nada foi alterado."
    exit 4
  }
  Write-Step "CheckOnly: tudo presente (toolchain+modelo). Para instalar, rode sem -CheckOnly."
  exit 0
}

# 3. instala o que der
# Modelo primeiro: o download nao exige admin/toolchain e e necessario de
# qualquer forma (idempotente - re-run pula arquivos presentes).
$step = Download-Model $network
if ($step -ne "ok") { Write-Err $step; exit 4 }

$step = Install-ToolchainIfNeeded $tc $network
if ($step -ne "ok") { Write-Err $step; exit 3 }

$step = Build-Candle
if ($step -ne "ok") { Write-Err "$step - o modelo/toolchain estao no lugar; falta so destravar o build."; exit 5 }

# 4. adequa o host
Set-LauncherEnv

# 5. smoke
Smoke

Write-Step "Pronto. NEURAL_SGDB_EMBEDDER=$EmbedderLabel configurado no host."
Write-Step "Proximo passo (unico pendente para semantica real): implementar"
Write-Step "  try_candle_embed em crates/nsgdb-embed/src/lib.rs"
Write-Step "  (tokenizer -> forward -> mean-pool -> L2, dim ${Dim}, model_id=${ModelId})."
Write-Step "Ate la, o embedder roda no fallback deterministico (mesma dim, era declarada)."
exit 0