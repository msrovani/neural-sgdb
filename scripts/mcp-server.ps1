# Launcher stdio para o MCP neural-sgdb no Cursor (Windows).
# Ordem: .nsgdb/bin > mcp-release > release. Auto-build se cargo no PATH.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$Installed = Join-Path $Root ".nsgdb\bin\mcp_server.exe"
$AltBin = Join-Path $Root "target\mcp-release\release\examples\mcp_server.exe"
$Bin = Join-Path $Root "target\release\examples\mcp_server.exe"
$Source = Join-Path $Root "examples\mcp_server.rs"

function Test-BinaryFresh([string]$Path) {
    if (-not (Test-Path $Path)) { return $false }
    if (-not (Test-Path $Source)) { return $true }
    return (Get-Item $Source).LastWriteTime -le (Get-Item $Path).LastWriteTime
}

$Exe = $null
foreach ($c in @($Installed, $AltBin, $Bin)) {
    if (Test-BinaryFresh $c) { $Exe = $c; break }
}

if (-not $Exe) {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        Write-Error "Binario MCP ausente. Rode: powershell -File scripts/mcp-install.ps1"
    }
    [Console]::Error.WriteLine("[neural-sgdb] building MCP...")
    Push-Location $Root
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo build --release --example mcp_server --target-dir (Join-Path $Root "target\mcp-release")
        if ($LASTEXITCODE -ne 0) {
            [Console]::Error.WriteLine("[neural-sgdb] build failed (exit $LASTEXITCODE)")
        }
    } finally {
        $ErrorActionPreference = $prevEap
        Pop-Location
    }
    foreach ($c in @($Installed, $AltBin, $Bin)) {
        if (Test-Path $c) { $Exe = $c; break }
    }
    if (-not $Exe) { Write-Error "Build terminou mas binario MCP nao encontrado" }
}

$env:NEURAL_SGDB_DB = if ($env:NEURAL_SGDB_DB) { $env:NEURAL_SGDB_DB } else { Join-Path $Root ".nsgdb\memory.db" }
# ADR-0008: do not default to demo — unset = lexical L3, no fake BQ era.
if (-not $env:NEURAL_SGDB_DEFAULT_SCOPE) { $env:NEURAL_SGDB_DEFAULT_SCOPE = "project/neural-sgdb" }
& $Exe
