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
# Старый config.ps1 без этой строки тоже работает: пустой ключ — вопросы текстом выключены.
$env:ASSISTANT_API_KEY = if (Test-Path variable:AssistantApiKey) { $AssistantApiKey } else { '' }
# Optima Business (SPEC-31): без ключа QR банка и сверка выключены.
$env:OPTIMA_API_KEY = if (Test-Path variable:OptimaApiKey) { $OptimaApiKey } else { '' }
$env:OPTIMA_API_URL = if (Test-Path variable:OptimaApiUrl) { $OptimaApiUrl } else { '' }
# Сообщения клиентам (SPEC-32): без ключа и номера WhatsApp выключен; без адреса — ссылка на бот.
$env:WHATSAPP_TOKEN = if (Test-Path variable:WhatsAppToken) { $WhatsAppToken } else { '' }
$env:WHATSAPP_PHONE_ID = if (Test-Path variable:WhatsAppPhoneId) { $WhatsAppPhoneId } else { '' }
$env:WHATSAPP_API_URL = if (Test-Path variable:WhatsAppApiUrl) { $WhatsAppApiUrl } else { '' }
$env:PUBLIC_URL = if (Test-Path variable:PublicUrl) { $PublicUrl } else { '' }
# Несколько филиалов через Cloudflare Tunnel (SPEC-33): адрес клиента — из заголовка Cloudflare.
$env:TRUST_CF_HEADER = if ((Test-Path variable:TrustCfHeader) -and $TrustCfHeader) { '1' } else { '' }
$env:RUST_LOG = 'info'
while ($true) {
  # Через cmd: журнал пишется байтами программы (UTF-8), без перекодировки PowerShell.
  & cmd.exe /c "`"$Root\avtodom-server.exe`" >> `"$Root\logs\server.log`" 2>&1"
  Add-Content -Encoding UTF8 "$Root\logs\server.log" "$(Get-Date -Format s) сервер остановился, перезапуск через 5 секунд"
  Start-Sleep -Seconds 5
}
