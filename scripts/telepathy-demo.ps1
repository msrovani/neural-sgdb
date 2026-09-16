# Piloto telepatia: InMemory + dois ficheiros FileStorage (feature p2p).
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Push-Location $Root
try {
    Write-Host "[neural-sgdb] 1/2 p2p_telepathy (InMemory)..."
    & cargo run --release --example p2p_telepathy --features p2p
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "[neural-sgdb] 2/2 telepathy_two_db (FileStorage)..."
    & cargo run --release --example telepathy_two_db --features p2p
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    Write-Host "[neural-sgdb] OK - InMemory + 2-DB telepatia passaram"
} finally {
    Pop-Location
}
