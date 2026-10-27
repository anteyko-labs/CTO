# Ночная копия базы в C:\Avtodom\backups. В облако её уносит Google Drive: эта папка добавлена
# в «Синхронизацию папок компьютера» (задача работает от SYSTEM и не видит диск G:). Без Google Drive
# копия делается так же, только остаётся на этом компьютере.
# Хранение: 7 последних годных ночных — за каждый день, а также самая ранняя копия каждой из
# 5 последних недель и каждого из 4 последних месяцев (самой старой — не меньше 3 месяцев);
# копии перед обновлением и восстановлением — 30 дней, две последние — всегда. Если копия сегодня не удалась, старые не удаляются, а владелец получает
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
# Ночные: 7 последних годных — за каждый день; кроме них — самая ранняя годная копия каждой из
# 5 последних недель (текущая и 4 прежних) и каждого из 4 последних месяцев (текущий и 3 прежних).
# Самая ранняя копия недели и месяца не меняется изо дня в день, поэтому самой старой копии всегда
# не меньше 3 месяцев (89–120 дней) и всего их не больше 16. Остальные удаляются: в копиях лежат
# и фото накладных, 30 полных копий быстро заняли бы весь Google Диск (ADR-089).
# Раньше «месячной» оставалась самая поздняя копия месяца — и самая старая была 31–60 дней.
$keep = @{}
$good = @($dumps | Where-Object { $_.Name -match '^avtodom-\d{4}-\d{2}-\d{2}\.dump$' } | Sort-Object Name | Where-Object { -not (Test-Dump $_.FullName) })
# Имена с датой сортируются как даты: от старых к новым.
$newest = @($good | Sort-Object Name -Descending | Select-Object -First 7)
foreach ($f in $newest) { $keep[$f.Name] = $true }
$firstOfWeek = [ordered]@{}
$firstOfMonth = [ordered]@{}
foreach ($f in $good) {
  $day = [datetime]::ParseExact($f.Name.Substring(8, 10), 'yyyy-MM-dd', $null)
  $week = $day.AddDays(-(([int]$day.DayOfWeek + 6) % 7)).ToString('yyyy-MM-dd')
  $month = $day.ToString('yyyy-MM')
  # Первая встреченная копия недели или месяца — самая ранняя.
  if (-not $firstOfWeek.Contains($week)) { $firstOfWeek[$week] = $f.Name }
  if (-not $firstOfMonth.Contains($month)) { $firstOfMonth[$month] = $f.Name }
}
foreach ($w in @($firstOfWeek.Keys | Sort-Object -Descending | Select-Object -First 5)) { $keep[$firstOfWeek[$w]] = $true }
foreach ($m in @($firstOfMonth.Keys | Sort-Object -Descending | Select-Object -First 4)) { $keep[$firstOfMonth[$m]] = $true }
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
