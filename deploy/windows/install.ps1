# Установка Avtodom на компьютер точки (Windows 10/11). Запускать от имени администратора
# из папки распакованного пакета: powershell -ExecutionPolicy Bypass -File install.ps1
$ErrorActionPreference = 'Stop'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = 'C:\Avtodom'

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Запустите PowerShell от имени администратора'
}
if (-not (Test-Path "$Here\config.ps1")) { throw 'Нет config.ps1: скопируйте config.example.ps1 в config.ps1 и заполните' }
. "$Here\config.ps1"
if (-not (Test-Path "$PgBin\psql.exe")) { throw "Не найден PostgreSQL в $PgBin — установите PostgreSQL 16" }

Write-Host '1/6 Файлы программы' -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Root, "$Root\logs", "$Root\backups", "$Root\caddy-data" | Out-Null
Copy-Item -Force "$Here\avtodom-server.exe", "$Here\caddy.exe" $Root
Copy-Item -Force -Recurse "$Here\web" $Root
Copy-Item -Force "$Here\config.ps1", "$Here\start-server.ps1", "$Here\start-caddy.ps1", "$Here\backup.ps1", "$Here\restore.ps1" $Root
(Get-Content "$Here\Caddyfile" -Raw).Replace('SERVER_IP', $ServerIp) | Set-Content -Encoding UTF8 "$Root\Caddyfile"

Write-Host '2/6 База данных' -ForegroundColor Cyan
$env:PGPASSWORD = $PgSuperPassword
$exists = & "$PgBin\psql.exe" -U postgres -h 127.0.0.1 -tAc "select 1 from pg_roles where rolname = 'avtodom'"
if ($exists -ne '1') {
  & "$PgBin\psql.exe" -U postgres -h 127.0.0.1 -c "create role avtodom login password '$DbPassword'"
  & "$PgBin\psql.exe" -U postgres -h 127.0.0.1 -c "create database avtodom owner avtodom"
} else {
  Write-Host '   база уже есть — оставляем как есть'
}
Remove-Item Env:PGPASSWORD

Write-Host '3/6 Автозапуск сервера и HTTPS' -ForegroundColor Cyan
$sys = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$keep = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
$boot = New-ScheduledTaskTrigger -AtStartup
foreach ($t in @(@{ Name = 'Avtodom сервер'; Script = 'start-server.ps1' }, @{ Name = 'Avtodom HTTPS'; Script = 'start-caddy.ps1' })) {
  $a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Root\$($t.Script)`"" -WorkingDirectory $Root
  Register-ScheduledTask -TaskName $t.Name -Action $a -Trigger $boot -Principal $sys -Settings $keep -Force | Out-Null
}

Write-Host '4/6 Копия базы каждую ночь в 02:30' -ForegroundColor Cyan
$night = New-ScheduledTaskTrigger -Daily -At '02:30'
$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Root\backup.ps1`"" -WorkingDirectory $Root
Register-ScheduledTask -TaskName 'Avtodom копия базы' -Action $a -Trigger $night -Principal $sys -Settings (New-ScheduledTaskSettingsSet -StartWhenAvailable) -Force | Out-Null

Write-Host '5/6 Брандмауэр: HTTPS для кассы в сети магазина' -ForegroundColor Cyan
if (-not (Get-NetFirewallRule -DisplayName 'Avtodom HTTPS' -ErrorAction SilentlyContinue)) {
  New-NetFirewallRule -DisplayName 'Avtodom HTTPS' -Direction Inbound -Protocol TCP -LocalPort 443 -Action Allow -Profile Private, Domain | Out-Null
}

Write-Host '6/6 Запуск' -ForegroundColor Cyan
Start-ScheduledTask -TaskName 'Avtodom сервер'
Start-ScheduledTask -TaskName 'Avtodom HTTPS'
Start-Sleep -Seconds 8
try {
  $r = Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:8080/' -TimeoutSec 10
  Write-Host "   сервер отвечает: $($r.StatusCode)" -ForegroundColor Green
} catch {
  Write-Host "   сервер пока не отвечает — смотрите $Root\logs\server.log" -ForegroundColor Yellow
}
$cert = "$Root\caddy-data\pki\authorities\local\root.crt"
Write-Host ''
Write-Host "Готово. Касса: https://$ServerIp" -ForegroundColor Green
Write-Host "Сертификат для моноблока появится здесь через минуту: $cert"
Write-Host 'Перенесите его на моноблок и запустите там install-cert.ps1 (см. README).'
