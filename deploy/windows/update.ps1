# Обновление Avtodom: копия базы → остановка → замена программы и экранов → запуск.
# Запускать от имени администратора из папки нового распакованного пакета:
#   powershell -ExecutionPolicy Bypass -File update.ps1
$ErrorActionPreference = 'Stop'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = 'C:\Avtodom'
if (-not (Test-Path "$Here\avtodom-server.exe")) { throw 'Запускайте из папки нового пакета (рядом должен быть avtodom-server.exe)' }

Write-Host '1/4 Копия базы перед обновлением' -ForegroundColor Cyan
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$Root\backup.ps1"
if ($LASTEXITCODE -ne 0) { throw 'Копия базы не создалась — обновление остановлено, смотрите C:\Avtodom\logs\backup.log' }

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
  Write-Host 'Обновлено, сервер отвечает. Касса подхватит новую версию сама (плашка «Обновить»).' -ForegroundColor Green
} catch {
  Write-Host "Сервер не отвечает — смотрите $Root\logs\server.log. Откат: restore.ps1 с копией из $Root\backups" -ForegroundColor Yellow
}
