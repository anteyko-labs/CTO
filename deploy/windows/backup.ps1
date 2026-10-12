# Ночная копия базы: в C:\Avtodom\backups и в папку Google Drive; храним 30 дней.
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
$env:PGPASSWORD = $DbPassword
$file = "$Root\backups\avtodom-$(Get-Date -Format 'yyyy-MM-dd').dump"
& "$PgBin\pg_dump.exe" -U avtodom -h 127.0.0.1 -d avtodom -Fc -f $file
if ($LASTEXITCODE -ne 0) { Add-Content "$Root\logs\backup.log" "$(Get-Date -Format s) ОШИБКА копии"; exit 1 }
if ($CloudBackupDir -and (Test-Path (Split-Path -Parent $CloudBackupDir))) {
  New-Item -ItemType Directory -Force -Path $CloudBackupDir | Out-Null
  Copy-Item -Force $file $CloudBackupDir
  Get-ChildItem $CloudBackupDir -Filter 'avtodom-*.dump' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-30) } | Remove-Item -Force
}
Get-ChildItem "$Root\backups" -Filter 'avtodom-*.dump' | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-30) } | Remove-Item -Force
Add-Content "$Root\logs\backup.log" "$(Get-Date -Format s) копия готова: $file"
