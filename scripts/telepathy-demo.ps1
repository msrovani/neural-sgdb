# Piloto telepatia: duas instancias Sgdb (InMemory) + CRDT — feature p2p.
# Nao usa o DB do MCP; prova sync entre nos distintos.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Push-Location $Root
try {
    Write-Host "[neural-sgdb] telepatia demo (p2p_telepathy)..."
    & cargo run --release --example p2p_telepathy --features p2p
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    Write-Host "[neural-sgdb] OK - ver docs/telepathy-pt.md"
} finally {
    Pop-Location
}
