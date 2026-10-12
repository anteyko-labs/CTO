# HTTPS внутри магазина: Caddy со своим сертификатом (касса без интернета требует HTTPS).
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
while ($true) {
  & "$Root\caddy.exe" run --config "$Root\Caddyfile" --adapter caddyfile *>> "$Root\logs\caddy.log"
  Start-Sleep -Seconds 5
}
