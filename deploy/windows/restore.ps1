# Восстановление базы из копии. Сервер останавливается на время восстановления.
# Пример: powershell -ExecutionPolicy Bypass -File restore.ps1 -Dump 'C:\Avtodom\backups\avtodom-2026-10-12.dump'
param([Parameter(Mandatory = $true)][string]$Dump)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
if (-not (Test-Path $Dump)) { throw "Нет файла $Dump" }
Stop-ScheduledTask -TaskName 'Avtodom сервер'
Get-Process avtodom-server -ErrorAction SilentlyContinue | Stop-Process -Force
$env:PGPASSWORD = $DbPassword
& "$PgBin\pg_restore.exe" -U avtodom -h 127.0.0.1 -d avtodom --clean --if-exists --no-owner $Dump
Start-ScheduledTask -TaskName 'Avtodom сервер'
Write-Host 'База восстановлена, сервер запущен' -ForegroundColor Green
