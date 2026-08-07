@echo off
title Discord Lyrics Status — Signing Setup
chcp 65001 >nul

echo ┌─────────────────────────────────────────────────────────────┐
echo │  Discord Lyrics Status — Configuração de Assinatura       │
echo │                                                           │
echo │  Gera um certificado auto-assinado para TESTES.           │
echo │  ⚠️  NÃO use em produção — usuários verão "Editor         │
echo │       desconhecido" pois não é uma CA confiável.           │
echo └─────────────────────────────────────────────────────────────┘
echo.

:: ── Verifica requisitos ────────────────────────────────────────────────
echo [1/3] Verificando requisitos...

where dotnet >nul 2>&1
if %errorlevel% neq 0 (
    echo ❌ .NET SDK nao encontrado!
    echo    Instale em: https://dotnet.microsoft.com/download
    pause
    exit /b 1
)

:: Verifica se o .NET ILRepack está disponível ou usa OpenSSL
where openssl >nul 2>&1
if %errorlevel% neq 0 (
    echo ⚠️  OpenSSL nao encontrado. Usando PowerShell para gerar o certificado.
) else (
    echo ✓ OpenSSL encontrado
)

echo ✓ .NET SDK encontrado
echo.

:: ── Gera certificado auto-assinado ────────────────────────────────────
echo [2/3] Gerando certificado auto-assinado...
echo.

set CERT_DIR=build\certs
if not exist %CERT_DIR% mkdir %CERT_DIR%

:: Remove certificados antigos
if exist %CERT_DIR%\test-cert.pfx del %CERT_DIR%\test-cert.pfx
if exist %CERT_DIR%\test-cert.cer del %CERT_DIR%\test-cert.cer

set CERT_PASSWORD=discord-lyrics-status-test-2026

:: Tenta com OpenSSL primeiro
where openssl >nul 2>&1
if %errorlevel% equ 0 (
    echo Gerando com OpenSSL...

    :: Gera chave privada e certificado auto-assinado
    openssl req -x509 -newkey rsa:4096 -keyout %CERT_DIR%\test-key.pem ^
        -out %CERT_DIR%\test-cert.pem -days 3650 -nodes ^
        -subj "/CN=Discord Lyrics Status Test/O=DiscordLyricsStatus/C=BR" ^
        -extensions v3_req ^
        -addext "keyUsage=digitalSignature" ^
        -addext "extendedKeyUsage=codeSigning"

    :: Converte para PFX
    openssl pkcs12 -export -out %CERT_DIR%\test-cert.pfx ^
        -inkey %CERT_DIR%\test-key.pem ^
        -in %CERT_DIR%\test-cert.pem ^
        -passout pass:%CERT_PASSWORD%

    :: Gera .cer para instalação
    openssl x509 -in %CERT_DIR%\test-cert.pem -outform der -out %CERT_DIR%\test-cert.cer

    :: Limpa arquivos temporários
    del %CERT_DIR%\test-key.pem %CERT_DIR%\test-cert.pem >nul 2>&1

    echo ✓ Certificado gerado com OpenSSL
    goto :INSTALL_CERT
)

:: Fallback: PowerShell (New-SelfSignedCertificate)
echo Gerando com PowerShell...
powershell -Command ^
    $cert = New-SelfSignedCertificate ^
        -Subject "CN=Discord Lyrics Status Test,O=DiscordLyricsStatus,C=BR" ^
        -Type CodeSigning ^
        -CertStoreLocation "Cert:\CurrentUser\My" ^
        -KeyUsage DigitalSignature ^
        -KeyExportPolicy Exportable ^
        -NotAfter (Get-Date).AddYears(10); ^
    $password = ConvertTo-SecureString -String "%CERT_PASSWORD%" -Force -AsPlainText; ^
    Export-PfxCertificate -Cert $cert -FilePath "%CD%\%CERT_DIR%\test-cert.pfx" -Password $password; ^
    Export-Certificate -Cert $cert -FilePath "%CD%\%CERT_DIR%\test-cert.cer" -Type CERT; ^
    Write-Host "✓ Certificado gerado com PowerShell"

if %errorlevel% neq 0 (
    echo ❌ Falha ao gerar certificado!
    pause
    exit /b 1
)

:INSTALL_CERT
echo.
echo [3/3] Deseja instalar o certificado na store confiavel?
echo    Isso fara com que o Windows confie no certificado localmente,
echo    eliminando o aviso "Editor desconhecido" APENAS nesta maquina.
echo.
choice /C SN /M "Instalar certificado (recomendado)?"
if %errorlevel% equ 1 (
    echo Instalando certificado na store "Trusted Root Certification Authorities"...

    :: Só funciona como Administrador! Tenta elevar
    certutil -addstore -f Root "%CERT_DIR%\test-cert.cer" >nul 2>&1
    certutil -addstore -f TrustedPublisher "%CERT_DIR%\test-cert.cer" >nul 2>&1

    if %errorlevel% equ 0 (
        echo ✓ Certificado instalado com sucesso!
    ) else (
        echo ⚠️  Nao foi possivel instalar automaticamente.
        echo    Execute como Administrador ou instale manualmente:
        echo       certutil -addstore Root "%CERT_DIR%\test-cert.cer"
    )
) else (
    echo ⏭️  Instalacao pulada.
)

echo.
echo ┌─────────────────────────────────────────────────────────────┐
echo │  ✅ Certificado criado!                                    │
echo │                                                             │
echo │  📁 Pasta: %CERT_DIR%\                            │
echo │  📄 PFX:   test-cert.pfx                                  │
echo │  📄 CER:   test-cert.cer (para instalar em outras maquinas)│
echo │                                                             │
echo │  Para usar no build, configure as variaveis:               │
echo │                                                             │
echo │    set WIN_CSC_LINK=%CD%\%CERT_DIR%\test-cert.pfx       │
echo │    set WIN_CSC_KEY_PASSWORD=%CERT_PASSWORD%                │
echo │                                                             │
echo │  Ou use o script build-installer.bat que le estas vars.    │
echo │                                                             │
echo │  ⚠️  Lembrete: certificados auto-assinados funcionam       │
echo │      apenas localmente. Para distribuicao, adquira um      │
echo │      certificado Authenticode real (R$200-600/ano).        │
echo └─────────────────────────────────────────────────────────────┘
echo.

pause
