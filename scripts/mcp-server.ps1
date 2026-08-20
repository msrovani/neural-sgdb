# Launcher stdio para o MCP neural-sgdb no Cursor (Windows).
# Prefer target/mcp-release — rebuild nao conflita com .exe locked pelo MCP em execucao.
# Embedder HTTP: examples/embedder_http.rs — ver docs/MCP.md
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$AltBin = Join-Path $Root "target\mcp-release\release\examples\mcp_server.exe"
$Bin = Join-Path $Root "target\release\examples\mcp_server.exe"
$Exe = if (Test-Path $AltBin) { $AltBin } elseif (Test-Path $Bin) { $Bin } else {
    Write-Error @"
Binario nao encontrado. Rode:
  cargo build --release --example mcp_server --target-dir target/mcp-release
(ou sem --target-dir se o MCP estiver desligado)
"@
}
$env:NEURAL_SGDB_DB = Join-Path $Root ".nsgdb\memory.db"
if (-not $env:NEURAL_SGDB_EMBEDDER) { $env:NEURAL_SGDB_EMBEDDER = "demo" }
& $Exe
