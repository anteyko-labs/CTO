# Запускать на МОНОБЛОКЕ кассы от имени администратора, рядом положить root.crt с сервера.
# После этого браузер доверяет адресу https://<ip сервера> и касса работает без интернета.
# Ярлык «Avtodom касса» (рабочий стол и автозагрузка): Chrome окном кассы с --kiosk-printing —
# чеки печатаются сразу на принтер по умолчанию, без окна печати (ADR-127). Повторный запуск
# ничего не дублирует: сертификат и ярлыки перезаписываются.
param([string]$Url = '')
$ErrorActionPreference = 'Stop'
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$cert = Join-Path $Here 'root.crt'
if (-not (Test-Path $cert)) { throw 'Положите root.crt (с сервера: C:\Avtodom\caddy-data\pki\authorities\local\root.crt) рядом с этим файлом' }
Import-Certificate -FilePath $cert -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Write-Host 'Сертификат установлен.' -ForegroundColor Green

# Адрес кассы: параметр -Url или файл, который положил install.ps1 на сервере.
$urlFile = Join-Path $Here 'kassa-url.txt'
if (-not $Url -and (Test-Path $urlFile)) { $Url = (Get-Content $urlFile -Raw).Trim() }
if (-not $Url) {
  Write-Host 'Адрес кассы неизвестен — ярлык не создан. Запустите: install-cert.ps1 -Url https://<адрес сервера>' -ForegroundColor Yellow
  exit 0
}

$chrome = @(
  "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
  "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
  "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $chrome) {
  Write-Host 'Chrome не найден — установите Google Chrome и запустите этот файл ещё раз.' -ForegroundColor Yellow
  exit 0
}

# Без окна печати Chrome печатает на принтер по умолчанию Windows, а не на последний выбранный.
$policy = 'HKLM:\SOFTWARE\Policies\Google\Chrome'
New-Item -Path $policy -Force | Out-Null
Set-ItemProperty -Path $policy -Name 'PrintPreviewUseSystemDefaultPrinter' -Value 1 -Type DWord
# Chrome, оставшийся в фоне после закрытия окон, не принял бы флаг печати при следующем запуске.
Set-ItemProperty -Path $policy -Name 'BackgroundModeEnabled' -Value 0 -Type DWord

$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
$startup = [Environment]::GetFolderPath('CommonStartup')
foreach ($dir in @($desktop, $startup)) {
  $lnk = $shell.CreateShortcut((Join-Path $dir 'Avtodom касса.lnk'))
  $lnk.TargetPath = $chrome
  $lnk.Arguments = "--app=$Url --kiosk-printing"
  $lnk.WorkingDirectory = Split-Path -Parent $chrome
  $lnk.IconLocation = "$chrome,0"
  $lnk.Description = 'Касса Avtodom: печать без окна'
  $lnk.Save()
}
Write-Host "Ярлык «Avtodom касса» — на рабочем столе и в автозагрузке: $Url, печать без окна." -ForegroundColor Green
Write-Host 'Чековый принтер сделайте принтером по умолчанию (см. README, раздел 2). Закройте все окна Chrome и откройте кассу ярлыком.'
