# Windows Task Scheduler — roda host_scheduler a cada hora com lock cooperativo
# Instala: powershell -ExecutionPolicy Bypass -File scripts\nsgdb-scheduler.ps1 install
# Roda: nsgdb-scheduler.exe (compilado de examples/host_scheduler.rs)

param([string]$Action="run")

$TaskName = "nsgdb-scheduler"
$Exe = Join-Path $PSScriptRoot "..\target\release\examples\host_scheduler.exe"
$Lock = Join-Path $env:LOCALAPPDATA "nsgdb\scheduler.lock"

if ($Action -eq "install") {
  $A = New-ScheduledTaskAction -Execute $Exe
  $T = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Hours 1)
  $S = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  Register-ScheduledTask -TaskName $TaskName -Action $A -Trigger $T -Settings $S -Description "nsgdb expire/decay/consolidate/audit" -Force
  Write-Output "Task $TaskName instalada (a cada 1h, lock $Lock)"
  exit
}

# run com lock cooperativo (mesmo .connector.lock do connectors/README.md)
try { $f = [System.IO.File]::Open($Lock, 'Create', 'ReadWrite', 'None'); $f.Close() } catch { Write-Output "lock busy, skip"; exit 0 }
& $Exe
Remove-Item $Lock -ErrorAction SilentlyContinue
