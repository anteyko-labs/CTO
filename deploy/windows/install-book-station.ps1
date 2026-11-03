# Запускать на МОНОБЛОКЕ КАССЫ от имени администратора, под пользователем кассы (задача 027):
# станция книжки — второе окно Chrome, которое печатает масляную книжку на принтер наклеек, а касса
# печатает чеки на чековый принтер. Оба — без окна печати (--kiosk-printing).
#   install-book-station.ps1 -Url https://oilprime.org -ReceiptPrinter POS58 -BookPrinter POS80
# -ReceiptPrinter — часть имени очереди чекового принтера: его помнит профиль Chrome кассы.
# -BookPrinter — часть имени очереди принтера наклеек: его помнит профиль станции книжки.
# Перед запуском закройте Chrome полностью (и значок у часов): иначе он перезапишет настройку
# принтера. Профиль кассы не меняется — вход и чеки без сети в очереди остаются.
# После: «Настройки → Масляная книжка и печать» → «Книжку печатать на втором принтере».
param(
  [string]$Url = 'https://oilprime.org',
  [string]$ReceiptPrinter = 'POS58',
  [string]$BookPrinter = 'POS80'
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
if (Get-Process chrome -ErrorAction SilentlyContinue) {
  Write-Host 'Chrome открыт — закройте все окна Chrome (и значок у часов) и запустите этот файл ещё раз.' -ForegroundColor Yellow
  exit 1
}

# Два принтера: каждый профиль Chrome печатает на последний выбранный в нём принтер, а не на
# принтер по умолчанию Windows (как на складе, ADR-183).
$policy = 'HKLM:\SOFTWARE\Policies\Google\Chrome'
New-Item -Path $policy -Force | Out-Null
Set-ItemProperty -Path $policy -Name 'PrintPreviewUseSystemDefaultPrinter' -Value 0 -Type DWord
Set-ItemProperty -Path $policy -Name 'BackgroundModeEnabled' -Value 0 -Type DWord

# Профилю Chrome — «последний выбранный принтер» (то, что Chrome запоминает после печати).
function Set-StickyPrinter([string]$dir, [string]$part) {
  $q = Get-Printer | Where-Object { $_.Name -like "*$part*" } | Select-Object -First 1
  if (-not $q) {
    Write-Host "Принтер «$part» не найден — откройте окно без --kiosk-printing, Ctrl+P, выберите принтер и напечатайте." -ForegroundColor Yellow
    return
  }
  $prefDir = Join-Path $dir 'Default'
  New-Item -ItemType Directory -Path $prefDir -Force | Out-Null
  $prefFile = Join-Path $prefDir 'Preferences'
  if (Test-Path $prefFile) { Copy-Item $prefFile "$prefFile.bak-avtodom" -Force }
  $pref = if (Test-Path $prefFile) { Get-Content $prefFile -Raw | ConvertFrom-Json } else { [pscustomobject]@{} }
  $app = @{ version = 2; recentDestinations = @(@{ id = $q.Name; origin = 'local'; account = '' }); isHeaderFooterEnabled = $false } | ConvertTo-Json -Compress -Depth 5
  $sticky = [pscustomobject]@{ appState = $app }
  if ($pref.PSObject.Properties['printing']) {
    $pref.printing | Add-Member -NotePropertyName 'print_preview_sticky_settings' -NotePropertyValue $sticky -Force
  } else {
    $pref | Add-Member -NotePropertyName 'printing' -NotePropertyValue ([pscustomobject]@{ print_preview_sticky_settings = $sticky }) -Force
  }
  $pref | ConvertTo-Json -Depth 50 -Compress | Set-Content -Path $prefFile -Encoding UTF8
  Write-Host "Профиль $dir печатает на «$($q.Name)»." -ForegroundColor Green
}

$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
$startup = [Environment]::GetFolderPath('CommonStartup')

# Касса: свой профиль пользователя (вход и очередь чеков без сети остаются), печать без окна.
Set-StickyPrinter (Join-Path $env:LOCALAPPDATA 'Google\Chrome\User Data') $ReceiptPrinter
foreach ($dir in @($desktop, $startup)) {
  Get-ChildItem $dir -Filter '*.lnk' | ForEach-Object {
    $lnk = $shell.CreateShortcut($_.FullName)
    if ($lnk.Arguments -like '*--app=*' -and $lnk.Arguments -notlike '*customer-screen*' -and $lnk.Arguments -notlike '*label-station*' -and $lnk.Arguments -notlike '*--kiosk-printing*') {
      $lnk.Arguments = "$($lnk.Arguments) --kiosk-printing"
      $lnk.Save()
      Write-Host "Ярлык кассы «$($_.Name)»: печать без окна." -ForegroundColor Green
    }
  }
}

# Станция книжки: своё окно со своим профилем; окно за кассой не засыпает (таймеры не тормозятся).
$bookDir = Join-Path $env:ProgramData 'AvtodomBookStation'
New-Item -ItemType Directory -Path $bookDir -Force | Out-Null
Set-StickyPrinter $bookDir $BookPrinter
$station = $Url.TrimEnd('/') + '/label-station?size=book'
$flags = '--kiosk-printing --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows'
foreach ($dir in @($desktop, $startup)) {
  $lnk = $shell.CreateShortcut((Join-Path $dir 'Avtodom book.lnk'))
  $lnk.TargetPath = $chrome
  $lnk.Arguments = "--app=$station $flags --user-data-dir=`"$bookDir`""
  $lnk.WorkingDirectory = Split-Path -Parent $chrome
  $lnk.IconLocation = "$chrome,0"
  $lnk.Description = 'Avtodom: oil book station, kiosk printing'
  $lnk.Save()
}
Write-Host "Ярлык «Avtodom book» — на рабочем столе и в автозагрузке: $station." -ForegroundColor Green
Write-Host 'Откройте «Avtodom book», войдите (один раз) и оставьте окно открытым за кассой. Затем откройте кассу её ярлыком.'
Write-Host 'Включите: «Настройки → Масляная книжка и печать» → «Книжку печатать на втором принтере».'
