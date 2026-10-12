# Запускать на МОНОБЛОКЕ кассы от имени администратора, рядом положить root.crt с сервера.
# После этого браузер доверяет адресу https://<ip сервера> и касса работает без интернета.
$ErrorActionPreference = 'Stop'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cert = Join-Path $Here 'root.crt'
if (-not (Test-Path $cert)) { throw 'Положите root.crt (с сервера: C:\Avtodom\caddy-data\pki\authorities\local\root.crt) рядом с этим файлом' }
Import-Certificate -FilePath $cert -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Write-Host 'Сертификат установлен. Перезапустите браузер и откройте кассу по адресу сервера.' -ForegroundColor Green
