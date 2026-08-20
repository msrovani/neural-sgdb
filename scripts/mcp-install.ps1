# Instala o binario MCP em path fixo (.nsgdb/bin) — rebuild seguro com MCP rodando.
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$OutDir = Join-Path $Root ".nsgdb\bin"
$TargetDir = Join-Path $Root "target\mcp-release"
$Src = Join-Path $TargetDir "release\examples\mcp_server.exe"
$Alt = Join-Path $Root "target\mcp-release\release\examples\mcp_server.exe"
$Dst = Join-Path $OutDir "mcp_server.exe"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Push-Location $Root
# Native cargo stderr must not trip Stop (PowerShell 5.1 treats stderr as errors).
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo build --release --example mcp_server --target-dir $TargetDir
$cargoExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($cargoExit -ne 0) {
  [Console]::Error.WriteLine("[neural-sgdb] build failed (exit $cargoExit); using existing binary if present")
}
$pick = if (Test-Path $Src) { $Src } elseif (Test-Path $Alt) { $Alt } else { $null }
if (-not $pick) {
  Write-Error "Nenhum mcp_server.exe encontrado. Feche o MCP no Cursor e rode novamente."
}
Copy-Item -Force $pick $Dst
Write-Host "OK: $Dst"
Pop-Location
