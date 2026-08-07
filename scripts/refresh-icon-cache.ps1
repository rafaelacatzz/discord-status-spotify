Write-Host "[IconCache] Refreshing Windows icon cache..."

try {
  $localAppData = $env:LOCALAPPDATA
  if (-not $localAppData) {
    throw "LOCALAPPDATA is not defined. Cannot refresh icon cache."
  }

  $explorerPath = Join-Path $localAppData "Microsoft\Windows\Explorer"
  if (-not (Test-Path $explorerPath)) {
    throw "Explorer icon cache folder not found: $explorerPath"
  }

  Get-ChildItem -Path $explorerPath -Filter "*iconcache*" -File -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "[IconCache] Removing $($_.FullName)"
    Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
  }

  Write-Host "[IconCache] Restarting Explorer..."
  Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 2
  Start-Process explorer.exe

  Write-Host "[IconCache] Done. If icons still appear stale, restart Windows."
  exit 0
} catch {
  Write-Error "[IconCache] Failed to refresh icon cache: $_"
  exit 1
}
