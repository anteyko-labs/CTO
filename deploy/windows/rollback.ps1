# Откат обновления: прежняя программа из C:\Avtodom\previous (её сохраняет update.ps1) и база из
# копии, сделанной перед обновлением. Всё, что внесено после обновления, пропадёт; копия текущей
# базы сохраняется (avtodom-ДАТА-before-restore-ЧЧММ.dump). Запускать от имени администратора:
#   powershell -ExecutionPolicy Bypass -File C:\Avtodom\rollback.ps1
$ErrorActionPreference = 'Stop'
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Запустите PowerShell от имени администратора'
}
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$prev = "$Root\previous"
if (-not (Test-Path "$prev\avtodom-server.exe") -or -not (Test-Path "$prev\dump.txt")) {
  throw "Нет прежней версии в $prev — откатывать не к чему"
}
$dump = "$(Get-Content -Encoding UTF8 "$prev\dump.txt" | Select-Object -First 1)".Trim()
if (-not (Test-Path -LiteralPath $dump)) { throw "Нет копии базы перед обновлением: $dump" }

Write-Host "Прежняя программа: $((Get-Item "$prev\avtodom-server.exe").LastWriteTime.ToString('dd.MM.yyyy HH:mm'))"
Write-Host "База на момент:    $((Get-Item -LiteralPath $dump).LastWriteTime.ToString('dd.MM.yyyy HH:mm')) ($dump)"
Write-Host ''
Write-Host 'Всё, что внесено после обновления, пропадёт (копия текущей базы сохранится).' -ForegroundColor Yellow
$answer = Read-Host 'Чтобы откатить, введите ОТКАТ'
if ("$answer".Trim() -ne 'ОТКАТ') {
  Write-Host 'Отменено, ничего не изменено.'
  exit 1
}

Stop-ScheduledTask -TaskName 'Avtodom сервер'
Get-Process avtodom-server -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
# Новая программа уходит в previous-new: если откат не нужен, её можно вернуть.
$newer = "$Root\previous-new"
if (Test-Path $newer) { Remove-Item -Recurse -Force $newer }
New-Item -ItemType Directory -Force -Path $newer | Out-Null
Copy-Item -Force "$Root\avtodom-server.exe" $newer
if (Test-Path "$Root\web") { Copy-Item -Recurse -Force "$Root\web" $newer }

Copy-Item -Force "$prev\avtodom-server.exe" $Root
if (Test-Path "$Root\web") { Remove-Item -Recurse -Force "$Root\web" }
if (Test-Path "$prev\web") { Copy-Item -Recurse -Force "$prev\web" $Root }

# Восстановление спрашивать не будет — подтверждение уже получено; сервер запустит само.
& "$Root\restore.ps1" -Dump $dump -Yes
if ($LASTEXITCODE -ne 0) {
  Write-Host "Откат не завершён: база не восстановилась. Новая программа сохранена в $newer." -ForegroundColor Red
  exit 1
}
Write-Host "Откат выполнен: работает прежняя версия. Новая сохранена в $newer." -ForegroundColor Green
