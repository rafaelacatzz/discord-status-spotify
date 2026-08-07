# ─────────────────────────────────────────────────────────────────────────────
#  kill-app.ps1 — Encerra todos os processos do Discord Lyrics Status
# ─────────────────────────────────────────────────────────────────────────────
#  Uso:  npm run kill-app
#        (ou: powershell -ExecutionPolicy Bypass -File scripts/kill-app.ps1)
#
#  Por quê: o app minimiza para a bandeja ao clicar no X (não sai de verdade).
#  Se ele ficar rodando, o rebuild trava no rcedit com "Unable to commit
#  changes" (o .exe fica locked). Este script mata:
#    1. O app empacotado (discord-lyrics-status.exe)
#    2. O daemon SMTC (SmtcDaemon.exe)
#    3. O backend node (src/index.js / dist/backend.cjs) e o Electron em dev
#       (electron.exe) — apenas os processos cuja linha de comando pertence
#       a este projeto, para nunca derrubar outros apps do usuário.
# ─────────────────────────────────────────────────────────────────────────────

$ErrorActionPreference = 'SilentlyContinue'
$found = $false

Write-Host '[KILL] Procurando processos do Discord Lyrics Status...'

# 1) App empacotado / instalado
Get-Process -Name 'discord-lyrics-status' -ErrorAction SilentlyContinue | ForEach-Object {
  $found = $true
  Write-Host ("[KILL] Encerrando discord-lyrics-status.exe (PID {0})" -f $_.Id)
  Stop-Process -Id $_.Id -Force
}

# 2) Daemon SMTC (spotify media control)
Get-Process -Name 'SmtcDaemon' -ErrorAction SilentlyContinue | ForEach-Object {
  $found = $true
  Write-Host ("[KILL] Encerrando SmtcDaemon.exe (PID {0})" -f $_.Id)
  Stop-Process -Id $_.Id -Force
}

# 3) Backend node + Electron em dev — só os deste projeto (filtro por comando)
Get-CimInstance Win32_Process | Where-Object {
  $_.CommandLine -match 'discord-lyrics-status|discord-status-spotify|src[\\/]index\.js|dist[\\/]backend\.cjs' -or
  ($_.Name -eq 'electron.exe' -and $_.ExecutablePath -match 'discord-status-spotify')
} | ForEach-Object {
  $found = $true
  Write-Host ("[KILL] Encerrando {0} (PID {1})" -f $_.Name, $_.ProcessId)
  Stop-Process -Id $_.ProcessId -Force
}

# 4) Remove locks/URLs de sessoes ANTERIORES: o instance.lock fica no disco
#    apos kill forcado (o handler de exit do backend nao roda) e o PID
#    reciclado pelo Windows engana a checagem de processo vivo — o backend
#    novo falha no acquireLock e sai com code 0 ("Backend exited with code 0").
$dataDir = Join-Path $env:APPDATA 'discord-lyrics-status'
if ($env:APPDATA -and (Test-Path $dataDir)) {
  foreach ($file in @('instance.lock', 'server.url')) {
    $path = Join-Path $dataDir $file
    if (Test-Path $path) {
      Remove-Item $path -Force
      $found = $true
      Write-Host ("[KILL] Removido stale: {0}" -f $path)
    }
  }
}

# Pequena pausa para o Windows liberar os handles dos arquivos
Start-Sleep -Milliseconds 300

if (-not $found) {
  Write-Host '[KILL] Nenhum processo do app encontrado — pode rebuildar a vontade.'
} else {
  Write-Host '[KILL] Pronto! Rebuild pode rodar sem exe travado.'
}
