# Восстановление базы из копии. Запускать от имени администратора.
# Скрипт показывает файл, просит ввести ВОССТАНОВИТЬ, останавливает сервер и делает копию текущей
# базы avtodom-ДАТА-before-restore-ЧЧММ.dump (не удалась — ничего не трогает). Затем пересоздаёт
# базу и восстанавливает копию в чистую — так встаёт и копия прежней версии программы. Если
# восстановление упало, сервер не запускается и скрипт пишет, какой копией вернуть всё как было.
#   powershell -ExecutionPolicy Bypass -File restore.ps1 -Dump 'C:\Avtodom\backups\avtodom-2026-10-12.dump'
# Проверка копии без касания рабочей базы: восстановить во временную базу avtodom_check,
# посчитать продажи и удалить её (раз в месяц, см. README):
#   powershell -ExecutionPolicy Bypass -File restore.ps1 -Dump 'C:\Avtodom\backups\avtodom-2026-10-12.dump' -Check
param(
  [Parameter(Mandatory = $true)][string]$Dump,
  [switch]$Check,
  # Без вопроса «ВОССТАНОВИТЬ»: только для rollback.ps1, который спросил сам.
  [switch]$Yes
)
$ErrorActionPreference = 'Stop'
# Остановка задачи сервера и чтение C:\Avtodom (доступ только администраторам) — без прав упадёт на полпути.
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Запустите PowerShell от имени администратора'
}
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
if (-not (Test-Path -LiteralPath $Dump)) { throw "Нет файла $Dump" }
$item = Get-Item -LiteralPath $Dump

# Файл должен читаться как копия базы и содержать данные продаж.
$env:PGPASSWORD = $DbPassword
$list = & "$PgBin\pg_restore.exe" --list $item.FullName
if ($LASTEXITCODE -ne 0) { throw "Файл не читается как копия базы (pg_restore, код $LASTEXITCODE): $($item.FullName)" }
if (-not ($list | Where-Object { $_ -match 'TABLE DATA public sales( |$)' })) { throw "В файле нет данных таблицы продаж — это не копия базы Avtodom: $($item.FullName)" }

Write-Host "Копия:  $($item.Name)"
Write-Host "Дата:   $($item.LastWriteTime.ToString('dd.MM.yyyy HH:mm'))"
Write-Host "Размер: $([math]::Round($item.Length / 1KB)) КБ"

if ($Check) {
  # Рабочую базу и сервер не трогаем. Создать и удалить базу может только postgres.
  $db = 'avtodom_check'
  $env:PGPASSWORD = $PgSuperPassword
  & "$PgBin\dropdb.exe" -U postgres -h 127.0.0.1 --if-exists $db
  if ($LASTEXITCODE -ne 0) { throw "Не удалось удалить прежнюю проверочную базу $db (код $LASTEXITCODE)" }
  & "$PgBin\createdb.exe" -U postgres -h 127.0.0.1 -O avtodom $db
  if ($LASTEXITCODE -ne 0) { throw "Не удалось создать проверочную базу $db (код $LASTEXITCODE)" }
  $env:PGPASSWORD = $DbPassword
  & "$PgBin\pg_restore.exe" -U avtodom -h 127.0.0.1 -d $db --no-owner $item.FullName
  $code = $LASTEXITCODE
  $stats = ''
  if ($code -eq 0) {
    # Запрос латиницей: русский текст в аргументах psql на Windows искажается.
    $row = & "$PgBin\psql.exe" -U avtodom -h 127.0.0.1 -d $db -tA -F '|' -c "select count(*), coalesce(to_char(max(created_at) at time zone 'Asia/Bishkek', 'DD.MM.YYYY HH24:MI'), '-') from sales"
    if ($LASTEXITCODE -ne 0) { $code = $LASTEXITCODE } else {
      $parts = "$row".Split('|')
      $stats = "$($parts[0]) чеков, последний $($parts[1])"
    }
  }
  $env:PGPASSWORD = $PgSuperPassword
  & "$PgBin\dropdb.exe" -U postgres -h 127.0.0.1 --if-exists $db
  $dropCode = $LASTEXITCODE
  Remove-Item Env:PGPASSWORD
  if ($dropCode -ne 0) { Write-Host "Проверочная база $db не удалилась (код $dropCode) — удалите её вручную: dropdb -U postgres $db" -ForegroundColor Yellow }
  if ($code -ne 0) {
    Write-Host "ПРОВЕРКА НЕ ПРОШЛА: копия $($item.Name) не восстанавливается (код $code). Проверьте более раннюю копию и сообщите разработчику." -ForegroundColor Red
    exit 1
  }
  Write-Host "Проверка пройдена: копия восстанавливается, в ней $stats. Рабочая база не тронута." -ForegroundColor Green
  exit 0
}

Write-Host ''
Write-Host 'Рабочая база будет заменена содержимым этой копии. Всё, что внесено позже, пропадёт.' -ForegroundColor Yellow
if (-not $Yes) {
  $answer = Read-Host 'Чтобы продолжить, введите ВОССТАНОВИТЬ'
  if ("$answer".Trim() -ne 'ВОССТАНОВИТЬ') {
    Write-Host 'Отменено, ничего не изменено.'
    exit 1
  }
}

Stop-ScheduledTask -TaskName 'Avtodom сервер'
Get-Process avtodom-server -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2

# Копия текущей базы — своё имя, ночная её не перезапишет; латиницей, как в update.ps1.
# Без проверки размера: на новом компьютере база пустая, и её копия маленькая.
# Если такая уже есть (повторный запуск в ту же минуту, например возврат этой же копией) —
# ждём следующую минуту, чтобы не затереть её.
do {
  $safety = "$Root\backups\avtodom-$(Get-Date -Format 'yyyy-MM-dd')-before-restore-$(Get-Date -Format 'HHmm').dump"
  if (Test-Path $safety) { Start-Sleep -Seconds 5 }
} while (Test-Path $safety)
& "$PgBin\pg_dump.exe" -U avtodom -h 127.0.0.1 -d avtodom -Fc -f $safety
$code = $LASTEXITCODE
if ($code -ne 0 -or -not (Test-Path $safety)) {
  Start-ScheduledTask -TaskName 'Avtodom сервер'
  throw "Копия текущей базы не создалась (код $code) — восстановление отменено, база не тронута, сервер запущен"
}
Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) копия перед восстановлением: $safety"
Write-Host "Копия текущей базы: $safety"

# Восстанавливаем в чистую базу: копия прежней версии программы не ложится поверх базы новой
# версии (--clean падает на таблицах, которых в копии нет). Сервер при запуске сам доведёт
# базу до своей версии (ADR-089). Удалить и создать базу может только postgres.
$env:PGPASSWORD = $PgSuperPassword
& "$PgBin\dropdb.exe" -U postgres -h 127.0.0.1 --force --if-exists avtodom
$code = $LASTEXITCODE
if ($code -eq 0) {
  & "$PgBin\createdb.exe" -U postgres -h 127.0.0.1 -O avtodom avtodom
  $code = $LASTEXITCODE
}
if ($code -eq 0) {
  $env:PGPASSWORD = $DbPassword
  & "$PgBin\pg_restore.exe" -U avtodom -h 127.0.0.1 -d avtodom --no-owner $item.FullName
  $code = $LASTEXITCODE
}
Remove-Item Env:PGPASSWORD
if ($code -ne 0) {
  Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) ОШИБКА: восстановление из $($item.FullName) не удалось (код $code), сервер остановлен"
  Write-Host ''
  Write-Host "Восстановление завершилось с ошибкой ($code). База могла восстановиться не полностью — СЕРВЕР НЕ ЗАПУЩЕН." -ForegroundColor Red
  Write-Host 'Вернуть базу как было до восстановления:' -ForegroundColor Red
  Write-Host "  powershell -ExecutionPolicy Bypass -File $Root\restore.ps1 -Dump '$safety'" -ForegroundColor Red
  Write-Host '(после удачного восстановления сервер запустится сам)'
  exit 1
}
Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) база восстановлена из $($item.FullName)"
Start-ScheduledTask -TaskName 'Avtodom сервер'
Write-Host "База восстановлена из $($item.Name), сервер запущен. Копия прежней базы: $safety" -ForegroundColor Green
