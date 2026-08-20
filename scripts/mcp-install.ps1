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
try {
  cargo build --release --example mcp_server --target-dir $TargetDir 2>&1 | Out-Null
} catch {
  Write-Host "[neural-sgdb] build skipped/failed (exe may be locked); using existing binary" >&2
}
$pick = if (Test-Path $Src) { $Src } elseif (Test-Path $Alt) { $Alt } else { $null }
if (-not $pick) {
  Write-Error "Nenhum mcp_server.exe encontrado. Feche o MCP no Cursor e rode novamente."
}
Copy-Item -Force $pick $Dst
Write-Host "OK: $Dst"
Pop-Location
