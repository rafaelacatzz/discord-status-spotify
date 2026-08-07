@echo off
setlocal enabledelayedexpansion
title Discord Lyrics Status — Build Installer
chcp 65001 >nul

echo ┌─────────────────────────────────────────────────────────────┐
echo │  Discord Lyrics Status — Build Installer   v1.0.0         │
echo │                                                           │
echo │  📦 Gera: dist-electron\Discord Lyrics Status-Setup-*.exe │
echo └─────────────────────────────────────────────────────────────┘
echo.

:: ── Verifica Node.js ─────────────────────────────────────────────────
echo [1/5] Verificando requisitos...
where node >nul 2>&1
if %errorlevel% neq 0 (
    echo ❌ Node.js nao encontrado! Instale Node.js 18+ em: https://nodejs.org
    pause
    exit /b 1
)
node --version
echo ✓ Node.js encontrado

:: ── Verifica configuração de assinatura ──────────────────────────────
echo.
if "%WIN_CSC_LINK%"=="" if "%WIN_CSC_NAME%"=="" if "%AZURE_KEY_VAULT_URI%"=="" (
    echo ⚠️  Nenhuma configuracao de assinatura encontrada.
    echo    O instalador sera criado SEM assinatura digital.
    echo    O Windows SmartScreen pode alertar os usuarios.
    echo.
    echo    Para configurar:
    echo      set WIN_CSC_LINK=C:\caminho\para\seu-cert.pfx
    echo      set WIN_CSC_KEY_PASSWORD=senha
    echo.
    echo    Ou para testes: scripts\sign-setup.bat
    echo.
    choice /C CS /M "Continuar sem assinatura [C] ou Configurar agora [S]?"
    if !errorlevel! equ 2 (
        echo.
        call scripts\sign-setup.bat
        if errorlevel 1 (
            echo ⚠️  Configuracao falhou. Continuando sem assinatura...
        )
    )
) else if not "%WIN_CSC_LINK%"=="" (
    if exist "%WIN_CSC_LINK%" (
        echo 🔏 Certificado encontrado: %WIN_CSC_LINK%
        echo ✓ Assinatura digital ativa
    ) else (
        echo ⚠️  Arquivo de certificado nao encontrado: %WIN_CSC_LINK%
        echo    Continuando sem assinatura digital...
        set SKIP_SIGN=true
    )
) else (
    echo 🔏 Assinatura digital ativa
)
echo.

:: ── Instala dependências ─────────────────────────────────────────────
echo [2/5] Instalando dependencias...
call npm install
if %errorlevel% neq 0 (
    echo ❌ npm install falhou!
    pause
    exit /b 1
)
echo ✓ Dependencias instaladas
echo.

:: ── Gera ícones ─────────────────────────────────────────────────────
echo [3/5] Gerando icones...
node scripts/generate-icons.js
if %errorlevel% neq 0 (
    echo ⚠️  Geracao de icone falhou, continuando...
) else (
    echo ✓ Icones gerados
)
echo.

:: ── Compila SMTC Daemon ──────────────────────────────────────────────
echo [4/5] Compilando SMTC Daemon...
call npm run build:daemon
if %errorlevel% neq 0 (
    echo ⚠️  SMTC Daemon nao compilou. O instalador sera criado sem suporte SMTC.
    echo    Certifique-se de ter o .NET SDK 8.0+ instalado.
) else (
    echo ✓ SMTC Daemon compilado
)
echo.

:: ── Build Electron ──────────────────────────────────────────────────
echo [5/5] Compilando instalador com electron-builder...
echo.
echo ⏳ Isso pode levar alguns minutos. A saida sera em: dist-electron\
echo.

call node build.mjs electron
if %errorlevel% neq 0 (
    echo ❌ Build do instalador falhou!
    echo.
    echo Causas comuns:
    echo   - Falta de dependencias: npm install
    echo   - Erro de compilacao: verifique a saida acima
    echo   - Falta de espaco em disco
    pause
    exit /b 1
)

:: ── Mostra resultado final ───────────────────────────────────────────
echo.
echo ┌─────────────────────────────────────────────────────────────┐
echo │  ✅ Instalador criado com sucesso!                         │
echo │                                                             │

:: Verifica se foi assinado
if not "%SKIP_SIGN%"=="true" if exist "%WIN_CSC_LINK%" (
    echo │  🔏 Instalador ASSINADO digitalmente                    │
) else if not "%WIN_CSC_NAME%"=="" (
    echo │  🔏 Instalador ASSINADO digitalmente                    │
) else (
    echo │  ⚠️  Instalador NAO ASSINADO                             │
    echo │  ⚠️  SmartScreen pode alertar os usuarios                │
)

echo │                                                             │
echo │  📁 Pasta: %cd%\dist-electron\                             │
for %%f in ("%cd%\dist-electron\*.exe") do (
    set INSTALLER_FILE=%%~nxf
    echo │  📦 Arquivo: %%~nxf
    echo │  📏 Tamanho: %%~zf bytes
)
echo │                                                             │
echo │  Compartilhe o .exe com quem voce quiser!                  │
echo └─────────────────────────────────────────────────────────────┘
echo.

pause
