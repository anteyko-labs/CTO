# HTTPS внутри магазина: Caddy со своим сертификатом (касса без интернета требует HTTPS).
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
while ($true) {
  & cmd.exe /c "`"$Root\caddy.exe`" run --config `"$Root\Caddyfile`" --adapter caddyfile >> `"$Root\logs\caddy.log`" 2>&1"
  Start-Sleep -Seconds 5
}
