# Launcher stdio para o MCP neural-sgdb no Cursor (Windows).
# Usa binario release — evita depender de cargo no PATH do IDE.
# Embedder HTTP: examples/embedder_http.rs — ver docs/MCP.md
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path (Join-Path $Root "target\release\examples\mcp_server.exe"))) {
    Write-Error "Binario nao encontrado. Rode: cargo build --release --example mcp_server"
}
$env:NEURAL_SGDB_DB = Join-Path $Root ".nsgdb\memory.db"
if (-not $env:NEURAL_SGDB_EMBEDDER) { $env:NEURAL_SGDB_EMBEDDER = "demo" }
& (Join-Path $Root "target\release\examples\mcp_server.exe")
