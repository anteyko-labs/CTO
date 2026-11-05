# Запускать на МОНОБЛОКЕ КАССЫ от имени администратора, под пользователем кассы (задача 034):
# станция документов — окно Chrome, которое печатает документы A4 (акт сверки, расписка, договор,
# остатки, отчёты) на принтер HP, а касса по-прежнему печатает чеки на чековый принтер.
#   install-doc-station.ps1 -Url https://oilprime.org -Printer 'HP LaserJet'
# -Printer — часть имени очереди принтера A4: его помнит профиль станции документов.
# Сначала выкатить версию с задачей 034: прежняя откроет этот адрес как станцию большого принтера
# этикеток. Chrome закрывать не нужно: у станции свой профиль, профиль кассы не меняется.
# Ярлык кассы получает ?docs=station: откройте кассу этим ярлыком один раз, устройство запомнит.
param(
  [string]$Url = 'https://oilprime.org',
  [string]$Printer = 'HP LaserJet'
)
$ErrorActionPreference = 'Stop'
$base = $Url.TrimEnd('/')

$chrome = @(
  "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
  "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
  "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $chrome) {
  Write-Host 'Chrome не найден — установите Google Chrome и запустите этот файл ещё раз.' -ForegroundColor Yellow
  exit 1
}
$q = Get-Printer | Where-Object { $_.Name -like "*$Printer*" } | Select-Object -First 1
if (-not $q) {
  Write-Host "Принтер «$Printer» не найден — поставьте драйвер и запустите этот файл ещё раз." -ForegroundColor Yellow
  exit 1
}

# Каждый профиль Chrome печатает на последний выбранный в нём принтер (ADR-183).
$policy = 'HKLM:\SOFTWARE\Policies\Google\Chrome'
New-Item -Path $policy -Force | Out-Null
Set-ItemProperty -Path $policy -Name 'PrintPreviewUseSystemDefaultPrinter' -Value 0 -Type DWord

# Профиль станции: «последний выбранный принтер» — HP. Chrome работает без повышения прав.
$docDir = Join-Path $env:ProgramData 'AvtodomDocStation'
$prefDir = Join-Path $docDir 'Default'
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
$pref | ConvertTo-Json -Depth 50 -Compress | Set-Content -Path $prefFile -Encoding UTF8
& icacls $docDir /grant "*S-1-5-32-545:(OI)(CI)M" /T | Out-Null
Write-Host "Станция документов печатает на «$($q.Name)»." -ForegroundColor Green

$shell = New-Object -ComObject WScript.Shell
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
$startup = [Environment]::GetFolderPath('CommonStartup')

# Ярлык кассы: только окно этой программы, остальные --app-ярлыки (WhatsApp и др.) не трогаем.
$kassa = 0
foreach ($dir in @($desktop, $startup)) {
  Get-ChildItem $dir -Filter '*.lnk' | ForEach-Object {
    $lnk = $shell.CreateShortcut($_.FullName)
    $args0 = $lnk.Arguments
    if ($args0 -match ('--app=' + [regex]::Escape($base) + '/?(\s|$)')) {
      $lnk.Arguments = $args0 -replace ('--app=' + [regex]::Escape($base) + '/?(?=\s|$)'), "--app=$base/?docs=station"
      $lnk.Save()
      $kassa++
      Write-Host "Ярлык кассы «$($_.Name)»: документы — на принтер HP." -ForegroundColor Green
    } elseif ($args0 -like "*--app=$base/?docs=station*") {
      $kassa++
    }
  }
}
if ($kassa -eq 0) { Write-Host "Ярлык кассы с адресом $base не найден — проверьте адрес в -Url." -ForegroundColor Yellow }

# Окно станции за кассой не засыпает (таймеры не тормозятся).
$station = "$base/label-station?size=doc"
$flags = '--kiosk-printing --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows'
foreach ($dir in @($desktop, $startup)) {
  $lnk = $shell.CreateShortcut((Join-Path $dir 'Avtodom docs.lnk'))
  $lnk.TargetPath = $chrome
  $lnk.Arguments = "--app=$station $flags --user-data-dir=`"$docDir`""
  $lnk.WorkingDirectory = Split-Path -Parent $chrome
  $lnk.IconLocation = "$chrome,0"
  $lnk.Description = 'Avtodom: document station, kiosk printing'
  $lnk.Save()
}
Write-Host "Ярлык «Avtodom docs» — на рабочем столе и в автозагрузке: $station." -ForegroundColor Green
Write-Host 'Откройте «Avtodom docs», войдите (один раз) и оставьте окно открытым за кассой. Затем откройте кассу её ярлыком.'
