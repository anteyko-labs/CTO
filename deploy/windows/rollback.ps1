# Откат обновления: прежняя программа из C:\Avtodom\previous (её сохраняет update.ps1) и база из
# копии, сделанной перед обновлением. Всё, что внесено после обновления, пропадёт; копия текущей
# базы сохраняется (avtodom-ДАТА-before-restore-ЧЧММ.dump). Запускать от имени администратора:
#   powershell -ExecutionPolicy Bypass -File C:\Avtodom\rollback.ps1
# Порядок: проверка копии → база из копии (restore.ps1 без запуска сервера) → прежняя программа →
# запуск. Программа меняется только после удачного восстановления: не восстановилась база —
# остаётся новая программа, которая с базой новой версии и работает.
$ErrorActionPreference = 'Stop'
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Запустите PowerShell от имени администратора'
}
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
$prev = "$Root\previous"
$newer = "$Root\previous-new"
if (-not (Test-Path "$prev\avtodom-server.exe") -or -not (Test-Path "$prev\dump.txt")) {
  throw "Нет прежней версии в $prev — откатывать не к чему"
}
# Повторный запуск после удачного отката: в C:\Avtodom уже прежняя программа. Второй откат
# затёр бы новую программу в previous-new прежней.
if ((Get-FileHash "$Root\avtodom-server.exe").Hash -eq (Get-FileHash "$prev\avtodom-server.exe").Hash) {
  Write-Host "Откат уже выполнен: в $Root работает прежняя версия. Новая программа — в $newer." -ForegroundColor Yellow
  exit 1
}
$dump = "$(Get-Content -Encoding UTF8 "$prev\dump.txt" | Select-Object -First 1)".Trim()
if (-not (Test-Path -LiteralPath $dump)) { throw "Нет копии базы перед обновлением: $dump" }
# Копия читается и в ней есть продажи — до вопроса и до остановки сервера.
$env:PGPASSWORD = $DbPassword
$list = & "$PgBin\pg_restore.exe" --list $dump
$code = $LASTEXITCODE
Remove-Item Env:PGPASSWORD
if ($code -ne 0) { throw "Копия перед обновлением не читается (pg_restore, код $code): $dump — откат не выполнен, ничего не изменено" }
if (-not ($list | Where-Object { $_ -match 'TABLE DATA public sales( |$)' })) { throw "В копии нет данных продаж: $dump — откат не выполнен, ничего не изменено" }

Write-Host "Прежняя программа: $((Get-Item "$prev\avtodom-server.exe").LastWriteTime.ToString('dd.MM.yyyy HH:mm'))"
Write-Host "База на момент:    $((Get-Item -LiteralPath $dump).LastWriteTime.ToString('dd.MM.yyyy HH:mm')) ($dump)"
Write-Host ''
Write-Host 'Всё, что внесено после обновления, пропадёт (копия текущей базы сохранится).' -ForegroundColor Yellow
$answer = Read-Host 'Чтобы откатить, введите ОТКАТ'
if ("$answer".Trim() -ne 'ОТКАТ') {
  Write-Host 'Отменено, ничего не изменено.'
  exit 1
}

# База — первой: restore.ps1 остановит сервер, снимет копию текущей базы и восстановит копию
# перед обновлением, сервер не запустит (-NoStart).
$code = 0
try {
  & "$Root\restore.ps1" -Dump $dump -Yes -NoStart
  $code = $LASTEXITCODE
} catch {
  # Отказ до остановки сервера или до копии текущей базы: база не тронута.
  Write-Host "$($_.Exception.Message)" -ForegroundColor Red
  $code = 2
}
if ($code -eq 2) {
  Start-ScheduledTask -TaskName 'Avtodom сервер'
  Write-Host 'Откат не выполнен: база не тронута, работает новая версия (сервер запущен).' -ForegroundColor Red
  exit 1
}
if ($code -ne 0) {
  # База могла восстановиться не полностью; restore.ps1 напечатал, какой копией вернуть всё как
  # было. В C:\Avtodom — новая программа: она подходит к этой копии и запустится после неё.
  Write-Host 'Откат не завершён: база не восстановилась, сервер остановлен, программа не менялась (новая версия).' -ForegroundColor Red
  exit 1
}

# База прежней версии — теперь прежняя программа. Новая уходит в previous-new: если откат не
# нужен, её можно вернуть.
if (Test-Path $newer) { Remove-Item -Recurse -Force $newer }
New-Item -ItemType Directory -Force -Path $newer | Out-Null
Copy-Item -Force "$Root\avtodom-server.exe" $newer
if (Test-Path "$Root\web") { Copy-Item -Recurse -Force "$Root\web" $newer }

Copy-Item -Force "$prev\avtodom-server.exe" $Root
if (Test-Path "$Root\web") { Remove-Item -Recurse -Force "$Root\web" }
if (Test-Path "$prev\web") { Copy-Item -Recurse -Force "$prev\web" $Root }
Start-ScheduledTask -TaskName 'Avtodom сервер'
Write-Host "Откат выполнен: работает прежняя версия. Новая сохранена в $newer." -ForegroundColor Green
