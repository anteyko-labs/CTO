# Обновление Avtodom: копия базы → остановка → замена программы и экранов → запуск.
# Запускать от имени администратора из папки нового распакованного пакета:
#   powershell -ExecutionPolicy Bypass -File update.ps1
$ErrorActionPreference = 'Stop'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = 'C:\Avtodom'
# Остановка задачи и запись в C:\Avtodom (доступ только администраторам) — без прав упадёт на полпути.
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Запустите PowerShell от имени администратора'
}
if (-not (Test-Path "$Here\avtodom-server.exe")) { throw 'Запускайте из папки нового пакета (рядом должен быть avtodom-server.exe)' }
. "$Root\config.ps1"

Write-Host '1/4 Копия базы перед обновлением' -ForegroundColor Cyan
# Своё имя, отдельно от ночной копии того же дня: ночная её не перезапишет. Латиницей —
# pg_dump на Windows читает путь в кодировке системы, русские буквы в имени файла ненадёжны.
$dump = "$Root\backups\avtodom-$(Get-Date -Format 'yyyy-MM-dd')-before-update-$(Get-Date -Format 'HHmm').dump"
$env:PGPASSWORD = $DbPassword
& "$PgBin\pg_dump.exe" -U avtodom -h 127.0.0.1 -d avtodom -Fc -f $dump
$code = $LASTEXITCODE
Remove-Item Env:PGPASSWORD
if ($code -ne 0 -or -not (Test-Path $dump) -or (Get-Item $dump).Length -lt 1024) {
  throw "Копия базы не создалась (код $code) — обновление остановлено, сервер не трогали"
}
Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) копия перед обновлением: $dump"
Write-Host "   копия: $dump"

Write-Host '2/4 Остановка сервера' -ForegroundColor Cyan
# Задача держит цикл перезапуска — останавливаем её, затем сам процесс.
Stop-ScheduledTask -TaskName 'Avtodom сервер'
Get-Process avtodom-server -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

Write-Host '3/4 Новые файлы' -ForegroundColor Cyan
Copy-Item -Force "$Here\avtodom-server.exe" $Root
Remove-Item -Recurse -Force "$Root\web"
Copy-Item -Recurse -Force "$Here\web" $Root
foreach ($f in 'start-server.ps1', 'start-caddy.ps1', 'backup.ps1', 'restore.ps1', 'update.ps1') { Copy-Item -Force "$Here\$f" $Root }

Write-Host '4/4 Запуск' -ForegroundColor Cyan
Start-ScheduledTask -TaskName 'Avtodom сервер'
Start-Sleep -Seconds 8
try {
  Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:8080/' -TimeoutSec 10 | Out-Null
  Write-Host 'Обновлено, сервер отвечает. Касса подхватит новую версию сама: на пустой кассе обновится, на других экранах — плашка «Обновить».' -ForegroundColor Green
} catch {
  Write-Host "Сервер не отвечает — смотрите $Root\logs\server.log. Откат: прежний пакет и restore.ps1 -Dump '$dump'" -ForegroundColor Yellow
}
