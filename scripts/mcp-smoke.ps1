# Smoke test MCP (Windows) — 23 tools, era_report, health onboarding, structuredContent.
$ErrorActionPreference = "Continue"
$Root = Split-Path -Parent $PSScriptRoot
& (Join-Path $Root "scripts\mcp-install.ps1") | Out-Null
$Db = Join-Path $env:TEMP "neural-sgdb-mcp-smoke-$PID.db"
$env:NEURAL_SGDB_DB = $Db
$Bin = Join-Path $Root ".nsgdb\bin\mcp_server.exe"
$init = '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
$list = '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
$health = '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}}'
$raw = @($init, $list, $health) | & $Bin 2>&1
$lines = @($raw | Where-Object { $_ -is [string] -and $_.TrimStart().StartsWith("{") })
if ($lines.Count -lt 3) { throw "expected 3 JSON lines, got $($lines.Count): $raw" }
$listJson = $lines[1] | ConvertFrom-Json
$count = $listJson.result.tools.Count
if ($count -ne 23) { throw "tools/list: expected 23, got $count" }
$listJson.result.tools | Where-Object { $_.name -eq "era_report" } | Select-Object -First 1 | Out-Null
$healthRes = ($lines[2] | ConvertFrom-Json).result
$structured = $healthRes.structuredContent
if (-not $structured.mcp_tool_count -or $structured.mcp_tool_count -ne 23) {
  throw "health missing mcp_tool_count=23"
}
if (-not $structured.onboarding) { throw "health missing onboarding" }
Remove-Item -Force $Db -ErrorAction SilentlyContinue
Write-Host "MCP smoke OK ($count tools, structured health)"
