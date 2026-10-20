# Ночная копия базы в C:\Avtodom\backups. В облако её уносит Google Drive: эта папка добавлена
# в «Синхронизацию папок компьютера» (задача работает от SYSTEM и не видит диск G:). Без Google Drive
# копия делается так же, только остаётся на этом компьютере.
# Хранение: 7 последних годных ночных — за каждый день, затем по одной на неделю (4 недели)
# и на месяц (3 месяца); копии перед обновлением и восстановлением — 30 дней, две последние —
# всегда. Если копия сегодня не удалась, старые не удаляются, а владелец получает
# «backup.failed» в «Уведомлениях» и в Телеграме.
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
$Backups = "$Root\backups"
$Log = "$Root\logs\backup.log"
New-Item -ItemType Directory -Force -Path $Backups, "$Root\logs" | Out-Null

function Write-Log([string]$Text) {
  Add-Content -Encoding UTF8 $Log "$(Get-Date -Format s) $Text"
}

if (-not (Test-Path "$Root\config.ps1")) { Write-Log 'ОШИБКА: нет config.ps1 — копия не создана'; exit 1 }
. "$Root\config.ps1"
if (-not (Test-Path "$PgBin\pg_dump.exe")) { Write-Log "ОШИБКА: не найден pg_dump в $PgBin — копия не создана"; exit 1 }
$env:PGPASSWORD = $DbPassword

# Запись в журнал действий сервера: владелец увидит её в «Уведомлениях» и в Телеграме.
# Если база недоступна совсем, запись не пройдёт — тогда остаётся только backup.log.
function Send-Failure([string]$Reason, [string]$Action = 'backup.failed') {
  $sqlReason = $Reason.Replace("'", "''")
  $sql = "insert into audit_log (id, branch_id, action, entity, data) select gen_random_uuid(), id, '$Action', 'backup', jsonb_build_object('reason', '$sqlReason') from branches;"
  # Через файл в UTF-8 без BOM: русский текст в аргументах psql на Windows искажается.
  $sqlFile = "$Root\logs\backup-failed.sql"
  [IO.File]::WriteAllText($sqlFile, $sql, (New-Object Text.UTF8Encoding $false))
  $env:PGCLIENTENCODING = 'UTF8'
  $out = & "$PgBin\psql.exe" -U avtodom -h 127.0.0.1 -d avtodom -q -v ON_ERROR_STOP=1 -f $sqlFile 2>&1 | ForEach-Object { "$_" }
  $code = $LASTEXITCODE
  Remove-Item -Force $sqlFile -ErrorAction SilentlyContinue
  if ($code -ne 0) { Write-Log "ОШИБКА: не удалось сообщить владельцу (psql, код $code) $($out -join ' ')" }
}

function Stop-Backup([string]$Reason) {
  Write-Log "ОШИБКА: копия не создана — $Reason"
  Send-Failure $Reason
  exit 1
}

# Копия годна, если pg_restore читает её оглавление и в ней есть данные таблицы продаж
# (обрезанный файл или копия чужой базы проверку не пройдут). Пусто — годна, иначе причина.
function Test-Dump([string]$Path) {
  $list = & "$PgBin\pg_restore.exe" --list $Path 2>$null
  if ($LASTEXITCODE -ne 0) { return "pg_restore не читает файл (код $LASTEXITCODE)" }
  if (-not ($list | Where-Object { $_ -match 'TABLE DATA public sales( |$)' })) { return 'в копии нет данных таблицы продаж (sales)' }
  return ''
}

$file = "$Backups\avtodom-$(Get-Date -Format 'yyyy-MM-dd').dump"
# Сначала во временный файл: неудачная попытка не затирает удачную копию этого же дня.
$part = "$file.part"
Remove-Item -Force $part -ErrorAction SilentlyContinue
$out = & "$PgBin\pg_dump.exe" -U avtodom -h 127.0.0.1 -d avtodom -Fc -f $part 2>&1 | ForEach-Object { "$_" }
$code = $LASTEXITCODE
if ($code -ne 0 -or -not (Test-Path $part)) {
  Remove-Item -Force $part -ErrorAction SilentlyContinue
  Stop-Backup "pg_dump завершился с ошибкой (код $code) $(($out | Select-Object -First 3) -join ' ')"
}
$bad = Test-Dump $part
if ($bad) {
  Remove-Item -Force $part -ErrorAction SilentlyContinue
  Stop-Backup "файл не прошёл проверку: $bad"
}
try {
  Move-Item -Force -LiteralPath $part -Destination $file -ErrorAction Stop
} catch {
  Stop-Backup "не удалось сохранить $file ($($_.Exception.Message))"
}
Write-Log "копия готова: $file ($([math]::Round((Get-Item $file).Length / 1KB)) КБ)"

# Чистка — только после удачной копии.
$now = Get-Date
$dumps = @(Get-ChildItem $Backups -Filter 'avtodom-*.dump')
# Ночные, от новых к старым: 7 последних годных — за каждый день, затем по одной годной на
# неделю (4 недели) и на месяц (3 месяца). Остальные удаляются: в копиях лежат и фото накладных,
# 30 полных копий быстро заняли бы весь Google Диск (ADR-089).
$keep = @{}
$days = 0
$weeks = @{}
$months = @{}
foreach ($f in ($dumps | Where-Object { $_.Name -match '^avtodom-(\d{4}-\d{2}-\d{2})\.dump$' } | Sort-Object Name -Descending)) {
  $day = [datetime]::ParseExact($f.Name.Substring(8, 10), 'yyyy-MM-dd', $null)
  if (Test-Dump $f.FullName) { continue }
  if ($days -lt 7) { $keep[$f.Name] = $true; $days++; continue }
  $week = $day.AddDays(-(([int]$day.DayOfWeek + 6) % 7)).ToString('yyyy-MM-dd')
  if ($weeks.Count -lt 4 -and -not $weeks.ContainsKey($week)) { $weeks[$week] = $true; $keep[$f.Name] = $true; continue }
  $month = $day.ToString('yyyy-MM')
  if ($months.Count -lt 3 -and -not $months.ContainsKey($month)) { $months[$month] = $true; $keep[$f.Name] = $true; continue }
}
foreach ($f in ($dumps | Where-Object { $_.Name -match '^avtodom-\d{4}-\d{2}-\d{2}\.dump$' })) {
  # Негодные и лишние — старше недели; свежие не трогаем, даже если они не прошли проверку.
  if (-not $keep.ContainsKey($f.Name) -and $f.LastWriteTime -lt $now.AddDays(-7)) {
    Remove-Item -Force $f.FullName
    Write-Log "удалена старая копия: $($f.Name)"
  }
}
# Перед обновлением и восстановлением — 30 дней, но две последние остаются всегда (откат).
$special = @($dumps | Where-Object { $_.Name -match '^avtodom-\d{4}-\d{2}-\d{2}-before-(update|restore)-\d{4}\.dump$' } | Sort-Object LastWriteTime -Descending)
for ($i = 2; $i -lt $special.Count; $i++) {
  $f = $special[$i]
  if ($f.LastWriteTime -lt $now.AddDays(-30)) {
    Remove-Item -Force $f.FullName
    Write-Log "удалена старая копия: $($f.Name)"
  }
}
# Сколько занимают копии: больше 10 ГБ — владельцу в «Уведомления», чтобы место на Google
# Диске не кончилось молча.
$size = (Get-ChildItem $Backups -Filter 'avtodom-*.dump' | Measure-Object -Property Length -Sum).Sum
if ($size -gt 10GB) { Send-Failure "копии базы занимают $([math]::Round($size / 1GB, 1)) ГБ — проверьте место на Google Диске" 'backup.size' }
# Брошенные временные файлы (прерванная копия).
Get-ChildItem $Backups -Filter '*.part' | Where-Object { $_.LastWriteTime -lt $now.AddDays(-1) } | Remove-Item -Force
