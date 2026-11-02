# Запускать на ПК СКЛАДА от имени администратора (задача 006, ADR-177): станция этикеток.
# Ярлык Chrome окном https://<адрес>/label-station с --kiosk-printing — задания с телефонов
# печатаются сразу на принтер по умолчанию, без окна печати. Ярлык — на общем рабочем столе и в
# автозагрузке. Повторный запуск ничего не дублирует.
#   install-label-station.ps1 -Url https://oilprime.org -Printer 'Xprinter' -SmallPrinter 'XP-236B'
# -Printer — часть имени очереди большого принтера: она станет принтером по умолчанию.
# -SmallPrinter — часть имени очереди маленького принтера 30×20 (задача 014): второй ярлык
#   «Avtodom labels small» (/label-station?size=small) со своим профилем Chrome, который
#   помнит этот принтер. Без параметра — один ярлык, как раньше.
# -RemovePrinter — часть имени лишней очереди на том же порту (её удаляем, порт общий).
param(
  [string]$Url = 'https://oilprime.org',
  [string]$Printer = 'Xprinter',
  [string]$RemovePrinter = '',
  [string]$SmallPrinter = ''
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
  # По ssh (без сеанса на экране) Windows не даёт сменить принтер по умолчанию — не останавливаемся.
  try {
    Invoke-CimMethod -InputObject $p -MethodName SetDefaultPrinter -ErrorAction Stop | Out-Null
    Write-Host "Принтер по умолчанию: $($p.Name) ($($p.PortName))." -ForegroundColor Green
  } catch {
    Write-Host "Принтер по умолчанию не сменить отсюда — проверьте на экране склада, что это $($p.Name)." -ForegroundColor Yellow
  }
} else {
  Write-Host "Принтер «$Printer» не найден — поставьте драйвер и сделайте его принтером по умолчанию вручную." -ForegroundColor Yellow
}

# Два принтера (задача 014): каждый профиль Chrome печатает на последний выбранный в нём принтер,
# а не на принтер по умолчанию Windows. Один принтер — как раньше, по умолчанию Windows.
$policy = 'HKLM:\SOFTWARE\Policies\Google\Chrome'
New-Item -Path $policy -Force | Out-Null
Set-ItemProperty -Path $policy -Name 'PrintPreviewUseSystemDefaultPrinter' -Value ([int](-not $SmallPrinter)) -Type DWord
Set-ItemProperty -Path $policy -Name 'BackgroundModeEnabled' -Value 0 -Type DWord

# Профилю Chrome — «последний выбранный принтер» (то, что Chrome запоминает после печати).
function Set-StickyPrinter([string]$dir, [string]$part) {
  $q = Get-Printer | Where-Object { $_.Name -like "*$part*" } | Select-Object -First 1
  if (-not $q) {
    Write-Host "Принтер «$part» не найден — откройте ярлык без печати по кнопке, Ctrl+P, выберите принтер и напечатайте пробную." -ForegroundColor Yellow
    return
  }
  $prefDir = Join-Path $dir 'Default'
  New-Item -ItemType Directory -Path $prefDir -Force | Out-Null
  $prefFile = Join-Path $prefDir 'Preferences'
  $pref = if (Test-Path $prefFile) { Get-Content $prefFile -Raw | ConvertFrom-Json } else { [pscustomobject]@{} }
  $app = @{ version = 2; recentDestinations = @(@{ id = $q.Name; origin = 'local'; account = '' }); isHeaderFooterEnabled = $false } | ConvertTo-Json -Compress -Depth 5
  $sticky = [pscustomobject]@{ appState = $app }
  if ($pref.PSObject.Properties['printing']) {
    $pref.printing | Add-Member -NotePropertyName 'print_preview_sticky_settings' -NotePropertyValue $sticky -Force
  } else {
    $pref | Add-Member -NotePropertyName 'printing' -NotePropertyValue ([pscustomobject]@{ print_preview_sticky_settings = $sticky }) -Force
  }
  $pref | ConvertTo-Json -Depth 20 -Compress | Set-Content -Path $prefFile -Encoding UTF8
  Write-Host "Профиль $dir печатает на «$($q.Name)»." -ForegroundColor Green
}

# WScript.Shell на английской Windows не сохраняет ярлык с кириллицей в имени: имя — латиницей.
$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
$startup = [Environment]::GetFolderPath('CommonStartup')
function New-Station([string]$name, [string]$path, [string]$profileDir) {
  # Свой профиль Chrome: флаг печати действует, даже если обычный Chrome уже открыт.
  New-Item -ItemType Directory -Path $profileDir -Force | Out-Null
  $station = $Url.TrimEnd('/') + $path
  foreach ($dir in @($desktop, $startup)) {
    $lnk = $shell.CreateShortcut((Join-Path $dir "$name.lnk"))
    $lnk.TargetPath = $chrome
    $lnk.Arguments = "--app=$station --kiosk-printing --user-data-dir=`"$profileDir`""
    $lnk.WorkingDirectory = Split-Path -Parent $chrome
    $lnk.IconLocation = "$chrome,0"
    $lnk.Description = 'Avtodom: label station, kiosk printing'
    $lnk.Save()
  }
  Write-Host "Ярлык «$name» — на рабочем столе и в автозагрузке: $station, печать без окна." -ForegroundColor Green
}

$bigDir = Join-Path $env:ProgramData 'AvtodomLabelStation'
New-Station 'Avtodom labels' '/label-station' $bigDir
if ($SmallPrinter) {
  $smallDir = Join-Path $env:ProgramData 'AvtodomLabelStationSmall'
  New-Station 'Avtodom labels small' '/label-station?size=small' $smallDir
  Set-StickyPrinter $bigDir $Printer
  Set-StickyPrinter $smallDir $SmallPrinter
}
Write-Host 'Откройте ярлык(и), войдите администратором филиала, нажмите «Пробная этикетка» в каждом окне.'
Write-Host 'Размер этикетки под рулон — «Настройки → Этикетки», вкладки «Большой штрихкод» и «Маленький штрихкод» (владелец).'
