# validate.ps1 - Valida a sintaxe de todos os arquivos JS do projeto e,
# opcionalmente, roda o build do Electron. Funciona SEM Git Bash.
#
# Uso (na raiz do projeto, pelo PowerShell ou cmd):
#   powershell -ExecutionPolicy Bypass -File scripts\validate.ps1          # so sintaxe
#   powershell -ExecutionPolicy Bypass -File scripts\validate.ps1 -Build   # sintaxe + build electron
#
# Observacao: este arquivo e intencionalmente ASCII (sem acentos) para o
# Windows PowerShell 5.1 ler corretamente sem BOM UTF-8.
#
param(
  [switch]$Build
)

$ErrorActionPreference = 'Stop'
# Garante UTF-8 ao enviar texto para o stdin do node.exe
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$root = Split-Path -Parent $PSScriptRoot   # sobe de scripts/ para a raiz do projeto
Set-Location $root

# ── Localiza o Node.js (PATH primeiro, fallback para instalacao padrao) ──
$node = (Get-Command node -ErrorAction SilentlyContinue).Source
if (-not $node -and (Test-Path 'C:\Program Files\nodejs\node.exe')) {
  $node = 'C:\Program Files\nodejs\node.exe'
}
if (-not $node) {
  Write-Host "node.exe nao encontrado. Instale o Node.js 18+." -ForegroundColor Red
  exit 1
}
Write-Host "Usando Node: $node" -ForegroundColor Cyan

# ── Coleta os arquivos JS/CJS/MJS do projeto ─────────────────────────────
$files = @()
$files += Get-ChildItem -Path (Join-Path $root 'public')   -Filter *.js  -File
$files += Get-ChildItem -Path (Join-Path $root 'src')      -Filter *.js  -File
$files += Get-ChildItem -Path (Join-Path $root 'scripts')  -Filter *.js  -File
$files += Get-ChildItem -Path (Join-Path $root 'electron') -Filter *.cjs -File -ErrorAction SilentlyContinue
if (Test-Path (Join-Path $root 'build.mjs'))   { $files += Get-Item (Join-Path $root 'build.mjs') }
if (Test-Path (Join-Path $root 'main.cjs'))    { $files += Get-Item (Join-Path $root 'main.cjs') }
if (Test-Path (Join-Path $root 'preload.cjs')) { $files += Get-Item (Join-Path $root 'preload.cjs') }

# ── node --check em cada arquivo ─────────────────────────────────────────
if ($files.Count -eq 0) {
  Write-Host "Nenhum arquivo JS encontrado para validar." -ForegroundColor Yellow
  exit 1
}

$failed = 0
foreach ($f in $files) {
  $rel = $f.FullName.Substring($root.Length + 1)
  # public/*.js sao classic scripts do browser (tag <script> sem type=module).
  # O package.json raiz tem "type":"module", entao o node --check direto os
  # trataria como ESM (strict mode) e poderia gerar falso negativo.
  # Para esses, usa-se o modo CommonJS via stdin, que e o mais proximo do
  # parse que o browser faz.
  $isClassicScript = $rel.StartsWith('public\')

  if ($isClassicScript) {
    $text = [System.IO.File]::ReadAllText($f.FullName)
    $text | & $node --input-type=commonjs --check - 2>&1 | Out-Null
    $code = $LASTEXITCODE
  } else {
    # src/*.js e build.mjs sao ESM (type:module); electron/*.cjs e CJS.
    & $node --check $f.FullName 2>&1 | Out-Null
    $code = $LASTEXITCODE
  }

  if ($code -eq 0) {
    Write-Host "  OK    $rel" -ForegroundColor Green
  } else {
    Write-Host "  FAIL  $rel" -ForegroundColor Red
    # Re-exibe o erro real para diagnostico
    if ($isClassicScript) {
      $text | & $node --input-type=commonjs --check - 2>&1
    } else {
      & $node --check $f.FullName 2>&1
    }
    $failed++
  }
}

if ($failed -gt 0) {
  Write-Host ""
  Write-Host "$failed arquivo(s) com erro de sintaxe." -ForegroundColor Red
  exit 1
}
Write-Host ""
Write-Host "Todos os arquivos OK." -ForegroundColor Green

# ── Build opcional do Electron (SKIP_SIGN=true pula a assinatura) ────────
if ($Build) {
  Write-Host ""
  Write-Host "== Build Electron (SKIP_SIGN=true) ==" -ForegroundColor Cyan
  $env:SKIP_SIGN = 'true'
  & $node (Join-Path $root 'build.mjs') electron
  if ($LASTEXITCODE -ne 0) {
    Write-Host "Build falhou." -ForegroundColor Red
    exit $LASTEXITCODE
  }
  Write-Host "Build concluido com sucesso." -ForegroundColor Green
}
