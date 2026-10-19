
### `PROXY_OFF.bat`

:::writing{variant="standard" id="74106" title="PROXY_OFF.bat"}
@echo off
setlocal
title PROXY OFF - Restaurar rede
color 0C

echo.
echo ==========================================
echo       DESATIVANDO PROXY DO WINDOWS
echo ==========================================
echo.

:: Verificar privilegios de administrador
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERRO] Execute este arquivo como ADMINISTRADOR.
    echo.
    pause
    exit /b 1
)

:: ==================================================
:: 1 - Restaurar WinHTTP
:: ==================================================
echo [1/4] Restaurando WinHTTP...
netsh winhttp reset proxy

:: ==================================================
:: 2 - Desativar WinINET
:: ==================================================
echo [2/4] Desativando WinINET...

reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" ^
 /v ProxyEnable /t REG_DWORD /d 0 /f >nul

:: ==================================================
:: 3 - Remover variaveis de ambiente persistentes
:: ==================================================
echo [3/4] Removendo variaveis de ambiente...

reg delete "HKCU\Environment" /v HTTP_PROXY /f >nul 2>&1
reg delete "HKCU\Environment" /v HTTPS_PROXY /f >nul 2>&1
reg delete "HKCU\Environment" /v ALL_PROXY /f >nul 2>&1
reg delete "HKCU\Environment" /v NO_PROXY /f >nul 2>&1

:: ==================================================
:: 4 - Limpar DNS
:: ==================================================
echo [4/4] Limpando cache DNS...
ipconfig /flushdns >nul

echo.
echo ==========================================
echo           PROXY DESATIVADO
echo ==========================================
echo.
echo WinHTTP:
netsh winhttp show proxy
echo.
echo O Windows voltou para acesso direto.
echo.
echo IMPORTANTE:
echo Feche e abra novamente CMD/PowerShell e
echo os programas que estavam abertos.
echo.
pause
endlocal
:::