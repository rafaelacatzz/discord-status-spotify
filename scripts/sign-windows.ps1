param(
    [Parameter(Mandatory = $true)]
    [string]$Executable,

    [Parameter(Mandatory = $true)]
    [string]$PfxBase64,

    [string]$PfxPassword = "",

    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending |
    Select-Object -First 1 -ExpandProperty FullName

if (-not $signtool) {
    $signtool = (Get-Command signtool.exe -ErrorAction SilentlyContinue).Source
}

if (-not $signtool) {
    throw "signtool.exe não encontrado. Instale Windows 10/11 SDK."
}

$resolvedExecutable = (Resolve-Path $Executable).Path
$pfxPath = Join-Path $env:TEMP ("SpotifySync-sign-" + [guid]::NewGuid().ToString("N") + ".pfx")

try {
    [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($PfxBase64))

    $arguments = @(
        "sign",
        "/fd", "SHA256",
        "/f", $pfxPath,
        "/tr", $TimestampUrl,
        "/td", "SHA256"
    )

    if ($PfxPassword) {
        $arguments += @("/p", $PfxPassword)
    }

    $arguments += $resolvedExecutable

    & $signtool @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "signtool.exe terminou com código $LASTEXITCODE."
    }

    & $signtool verify /pa /all /v $resolvedExecutable
    if ($LASTEXITCODE -ne 0) {
        throw "Falha ao verificar a assinatura."
    }
}
finally {
    Remove-Item -LiteralPath $pfxPath -Force -ErrorAction SilentlyContinue
}
