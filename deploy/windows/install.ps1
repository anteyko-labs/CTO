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
foreach ($p in @(@{ N = 'PgSuperPassword'; V = $PgSuperPassword }, @{ N = 'DbPassword'; V = $DbPassword }, @{ N = 'OwnerPassword'; V = $OwnerPassword })) {
  # Только печатные латинские символы: русские буквы и пробелы ломают строку подключения,
  # а кавычка " и обратная косая \ — передачу аргументов psql в PowerShell 5.1.
  if ($p.V -notmatch '^[\x21-\x7E]{10,}$' -or $p.V -match '["\\]') {
    throw "Пароль $($p.N): латиница, цифры и знаки, без пробелов, кавычки `" и обратной косой \, не короче 10 символов"
  }
}

function Invoke-Pg([string[]]$PgArgs) {
  & "$PgBin\psql.exe" @PgArgs
  if ($LASTEXITCODE -ne 0) {
    # Пароль в команде (create role … password '…') в текст ошибки не попадает.
    $shown = ($PgArgs -join ' ') -replace "password\s+'(?:[^']|'')*'", "password '***'"
    throw "psql завершился с ошибкой ($LASTEXITCODE): $shown"
  }
}

Write-Host '1/6 Файлы программы' -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $Root, "$Root\logs", "$Root\backups", "$Root\caddy-data" | Out-Null
Copy-Item -Force "$Here\avtodom-server.exe", "$Here\caddy.exe" $Root
Copy-Item -Force -Recurse "$Here\web" $Root
Copy-Item -Force "$Here\config.ps1", "$Here\start-server.ps1", "$Here\start-caddy.ps1", "$Here\backup.ps1", "$Here\restore.ps1", "$Here\update.ps1" $Root
# Пароли и копии базы — только системе и администраторам (SID — имена групп зависят от языка Windows).
& icacls $Root /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Не удалось ограничить доступ к C:\Avtodom' }
# Папку копий читает Google Drive от имени пользователя — ему чтение (только этой папки).
& icacls "$Root\backups" /grant "$($env:USERDOMAIN)\$($env:USERNAME):(OI)(CI)RX" | Out-Null
(Get-Content "$Here\Caddyfile" -Raw).Replace('SERVER_IP', $ServerIp) | Set-Content -Encoding UTF8 "$Root\Caddyfile"

Write-Host '2/6 База данных' -ForegroundColor Cyan
$env:PGPASSWORD = $PgSuperPassword
$exists = & "$PgBin\psql.exe" -U postgres -h 127.0.0.1 -tAc "select 1 from pg_roles where rolname = 'avtodom'"
if ($LASTEXITCODE -ne 0) { throw 'Не удалось подключиться к PostgreSQL: проверьте пароль postgres в config.ps1' }
$sqlPass = $DbPassword.Replace("'", "''")
if ($exists -ne '1') {
  Invoke-Pg @('-U', 'postgres', '-h', '127.0.0.1', '-c', "create role avtodom login password '$sqlPass'")
  Invoke-Pg @('-U', 'postgres', '-h', '127.0.0.1', '-c', 'create database avtodom owner avtodom')
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
# Любой профиль сети (новую сеть Windows считает общественной), но только адреса этой же сети.
Get-NetFirewallRule -DisplayName 'Avtodom HTTPS' -ErrorAction SilentlyContinue | Remove-NetFirewallRule
New-NetFirewallRule -DisplayName 'Avtodom HTTPS' -Direction Inbound -Protocol TCP -LocalPort 443 -Action Allow -Profile Any -RemoteAddress LocalSubnet | Out-Null

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
# Корневой сертификат Caddy создаёт при первом запуске — ждём и кладём копию туда, где её
# можно взять на флешку без прав администратора.
$cert = "$Root\caddy-data\pki\authorities\local\root.crt"
for ($i = 0; $i -lt 30 -and -not (Test-Path $cert); $i++) { Start-Sleep -Seconds 2 }
$certOut = 'C:\Avtodom-сертификат'
if (Test-Path $cert) {
  New-Item -ItemType Directory -Force -Path $certOut | Out-Null
  Copy-Item -Force $cert "$certOut\root.crt"
  Copy-Item -Force "$Here\install-cert.ps1" $certOut
  # Адрес кассы для ярлыка на моноблоке (install-cert.ps1): печать без окна, ADR-127.
  Set-Content -Encoding UTF8 -Path "$certOut\kassa-url.txt" -Value "https://$ServerIp"
}
Write-Host ''
Write-Host "Готово. Касса: https://$ServerIp" -ForegroundColor Green
if (Test-Path "$certOut\root.crt") {
  Write-Host "Для моноблока: скопируйте папку $certOut на флешку и запустите там install-cert.ps1 (см. README) —"
  Write-Host '   он установит сертификат и ярлык кассы «Avtodom касса» с печатью без окна.'
} else {
  Write-Host "Сертификат ещё не создан — смотрите $Root\logs\caddy.log и запустите install.ps1 ещё раз" -ForegroundColor Yellow
}
Write-Host "Копии базы: $Root\backups — добавьте эту папку в Google Drive (см. README, шаг 1.9)."
