@echo off
setlocal
title PROXY ON - 10.71.145.245:8080
color 0A

echo.
echo ==========================================
echo       ATIVANDO PROXY DO WINDOWS
echo       10.71.145.245:8080
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
:: 1 - WinHTTP
:: Windows Update e varios servicos do Windows
:: ==================================================
echo [1/5] Configurando WinHTTP...
netsh winhttp set proxy proxy-server="http=10.71.145.245:8080;https=10.71.145.245:8080" bypass-list="localhost;127.0.0.1;*.local"

if %errorlevel% neq 0 (
echo [ERRO] Falha ao configurar WinHTTP.
)

:: ==================================================
:: 2 - WinINET
:: Internet Settings usados por varios programas
:: ==================================================
echo [2/5] Configurando WinINET...

reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" ^
/v ProxyEnable /t REG_DWORD /d 1 /f >nul

reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" ^
/v ProxyServer /t REG_SZ /d "10.71.145.245:8080" /f >nul

reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" ^
/v ProxyOverride /t REG_SZ /d "localhost;127.0.0.1;*.local" /f >nul

:: ==================================================
:: 3 - Variaveis de ambiente para programas/terminais
:: ==================================================
echo [3/5] Configurando variaveis de ambiente...

setx HTTP_PROXY "http://10.71.145.245:8080" >nul
setx HTTPS_PROXY "http://10.71.145.245:8080" >nul
setx ALL_PROXY "http://10.71.145.245:8080" >nul
setx NO_PROXY "localhost,127.0.0.1,*.local" >nul

:: ==================================================
:: 4 - Variaveis para o processo atual
:: ==================================================
echo [4/5] Configurando ambiente atual...

set "HTTP_PROXY=http://10.71.145.245:8080"
set "HTTPS_PROXY=http://10.71.145.245:8080"
set "ALL_PROXY=http://10.71.145.245:8080"
set "NO_PROXY=localhost,127.0.0.1,*.local"

:: Algumas ferramentas usam minusculas
set "http_proxy=http://10.71.145.245:8080"
set "https_proxy=http://10.71.145.245:8080"
set "all_proxy=http://10.71.145.245:8080"
set "no_proxy=localhost,127.0.0.1,*.local"

:: ==================================================
:: 5 - Atualizar cache DNS
:: ==================================================
echo [5/5] Limpando cache DNS...
ipconfig /flushdns >nul

echo.
echo ==========================================
echo           PROXY ATIVADO
echo ==========================================
echo.
echo Proxy HTTP : 10.71.145.245:8080
echo Proxy HTTPS: 10.71.145.245:8080
echo.
echo WinHTTP:
netsh winhttp show proxy
echo.
echo Variaveis persistentes foram configuradas.
echo.
echo IMPORTANTE:
echo - Abra um NOVO CMD/PowerShell.
echo - Programas ja abertos podem precisar ser
echo   fechados e iniciados novamente.
echo - Programas que ignoram proxy continuarao
echo   usando sua propria conexao.
echo.
pause
endlocal
