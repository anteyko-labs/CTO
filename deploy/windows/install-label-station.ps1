# Запускать на ПК СКЛАДА от имени администратора (задача 006, ADR-177): станция этикеток.
# Ярлык Chrome окном https://<адрес>/label-station с --kiosk-printing — задания с телефонов
# печатаются сразу на принтер по умолчанию, без окна печати. Ярлык — на общем рабочем столе и в
# автозагрузке. Повторный запуск ничего не дублирует.
#   install-label-station.ps1 -Url https://oilprime.org -Printer 'Xprinter' -RemovePrinter 'RONGTA'
# -Printer — часть имени очереди этикеточного принтера: она станет принтером по умолчанию.
# -RemovePrinter — часть имени лишней очереди на том же порту (её удаляем, порт общий).
param(
  [string]$Url = 'https://oilprime.org',
  [string]$Printer = 'Xprinter',
  [string]$RemovePrinter = ''
)
$ErrorActionPreference = 'Stop'

$chrome = @(
  "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
  "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
  "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $chrome) {
  Write-Host 'Chrome не найден — установите Google Chrome и запустите этот файл ещё раз.' -ForegroundColor Yellow
  exit 1
}

# Лишняя очередь на общем USB-порту перехватывала бы задания.
if ($RemovePrinter) {
  Get-Printer | Where-Object { $_.Name -like "*$RemovePrinter*" } | ForEach-Object {
    Remove-Printer -Name $_.Name
    Write-Host "Удалена очередь печати «$($_.Name)»." -ForegroundColor Green
  }
}

# Этикеточный принтер — по умолчанию: Chrome с --kiosk-printing печатает на него.
$p = Get-CimInstance Win32_Printer | Where-Object { $_.Name -like "*$Printer*" } | Select-Object -First 1
if ($p) {
  Invoke-CimMethod -InputObject $p -MethodName SetDefaultPrinter | Out-Null
  Write-Host "Принтер по умолчанию: $($p.Name) ($($p.PortName))." -ForegroundColor Green
} else {
  Write-Host "Принтер «$Printer» не найден — поставьте драйвер и сделайте его принтером по умолчанию вручную." -ForegroundColor Yellow
}

# Без окна печати Chrome печатает на принтер по умолчанию Windows, а не на последний выбранный.
$policy = 'HKLM:\SOFTWARE\Policies\Google\Chrome'
New-Item -Path $policy -Force | Out-Null
Set-ItemProperty -Path $policy -Name 'PrintPreviewUseSystemDefaultPrinter' -Value 1 -Type DWord
Set-ItemProperty -Path $policy -Name 'BackgroundModeEnabled' -Value 0 -Type DWord

# Свой профиль Chrome: флаг печати действует, даже если обычный Chrome уже открыт.
$profileDir = Join-Path $env:ProgramData 'AvtodomLabelStation'
New-Item -ItemType Directory -Path $profileDir -Force | Out-Null
$station = $Url.TrimEnd('/') + '/label-station'

# WScript.Shell на английской Windows не сохраняет ярлык с кириллицей в имени: имя — латиницей.
$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
$startup = [Environment]::GetFolderPath('CommonStartup')
foreach ($dir in @($desktop, $startup)) {
  $lnk = $shell.CreateShortcut((Join-Path $dir 'Avtodom labels.lnk'))
  $lnk.TargetPath = $chrome
  $lnk.Arguments = "--app=$station --kiosk-printing --user-data-dir=`"$profileDir`""
  $lnk.WorkingDirectory = Split-Path -Parent $chrome
  $lnk.IconLocation = "$chrome,0"
  $lnk.Description = 'Avtodom: label station, kiosk printing'
  $lnk.Save()
}
Write-Host "Ярлык «Avtodom labels» — на рабочем столе и в автозагрузке: $station, печать без окна." -ForegroundColor Green
Write-Host 'Откройте ярлык, войдите администратором филиала, нажмите «Пробная этикетка».'
Write-Host 'Размер этикетки под рулон — «Настройки → Этикетки» (владелец).'
