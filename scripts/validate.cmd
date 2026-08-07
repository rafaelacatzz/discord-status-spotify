@echo off
REM validate.cmd - roda scripts\validate.ps1 via PowerShell, sem precisar de Git Bash.
REM Duplo-clique para validar a sintaxe; ou: validate.cmd -Build para validar + build.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0validate.ps1" %*
if errorlevel 1 (
  echo.
  echo VALIDACAO FALHOU - veja os erros acima.
  echo.
)
pause
