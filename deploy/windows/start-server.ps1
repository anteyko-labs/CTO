# Запуск сервера Avtodom: настройки из config.ps1, при падении — перезапуск через 5 секунд.
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
$pass = [uri]::EscapeDataString($DbPassword)
$env:DATABASE_URL = "postgres://avtodom:$pass@127.0.0.1:5432/avtodom"
$env:BIND_ADDR = '127.0.0.1:8080'
$env:WEB_DIR = "$Root\web"
$env:COOKIE_SECURE = 'true'
$env:BOOTSTRAP_OWNER_LOGIN = $OwnerLogin
$env:BOOTSTRAP_OWNER_PASSWORD = $OwnerPassword
$env:TELEGRAM_BOT_TOKEN = $TelegramToken
$env:RUST_LOG = 'info'
while ($true) {
  # Через cmd: журнал пишется байтами программы (UTF-8), без перекодировки PowerShell.
  & cmd.exe /c "`"$Root\avtodom-server.exe`" >> `"$Root\logs\server.log`" 2>&1"
  Add-Content -Encoding UTF8 "$Root\logs\server.log" "$(Get-Date -Format s) сервер остановился, перезапуск через 5 секунд"
  Start-Sleep -Seconds 5
}
