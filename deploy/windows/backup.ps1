# Ночная копия базы в C:\Avtodom\backups, храним 30 дней. В облако её уносит Google Drive:
# эта папка добавлена в «Синхронизацию папок компьютера» (задача работает от SYSTEM и не видит диск G:).
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
$env:PGPASSWORD = $DbPassword
$file = "$Root\backups\avtodom-$(Get-Date -Format 'yyyy-MM-dd').dump"
& "$PgBin\pg_dump.exe" -U avtodom -h 127.0.0.1 -d avtodom -Fc -f $file
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $file) -or (Get-Item $file).Length -lt 1024) {
  Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) ОШИБКА: копия не создана (код $LASTEXITCODE)"
  exit 1
}
Get-ChildItem "$Root\backups" -Filter 'avtodom-*.dump' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-30) } | Remove-Item -Force
Add-Content -Encoding UTF8 "$Root\logs\backup.log" "$(Get-Date -Format s) копия готова: $file ($([math]::Round((Get-Item $file).Length / 1KB)) КБ)"
