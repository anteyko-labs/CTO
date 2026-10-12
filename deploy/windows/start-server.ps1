# Запуск сервера Avtodom: настройки из config.ps1, при падении — перезапуск через 5 секунд.
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$Root\config.ps1"
$env:DATABASE_URL = "postgres://avtodom:$DbPassword@127.0.0.1:5432/avtodom"
$env:BIND_ADDR = '127.0.0.1:8080'
$env:WEB_DIR = "$Root\web"
$env:COOKIE_SECURE = 'true'
$env:BOOTSTRAP_OWNER_LOGIN = $OwnerLogin
$env:BOOTSTRAP_OWNER_PASSWORD = $OwnerPassword
$env:TELEGRAM_BOT_TOKEN = $TelegramToken
$env:RUST_LOG = 'info'
while ($true) {
  & "$Root\avtodom-server.exe" *>> "$Root\logs\server.log"
  Add-Content "$Root\logs\server.log" "$(Get-Date -Format s) сервер остановился, перезапуск через 5 секунд"
  Start-Sleep -Seconds 5
}
