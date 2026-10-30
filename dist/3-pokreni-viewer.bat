@echo off
title Remote Desk - VIEWER (gleda i kontrolise host)
cd /d "%~dp0"
echo ============================================================
echo  REMOTE DESK - VIEWER
echo ============================================================
echo.
echo Prvo pokreni host na drugom racunaru (2-pokreni-host.bat); on ti pokaze svoju IP adresu.
echo.
set /p IP=Upisi IP adresu hosta (npr. 192.168.100.72) i pritisni Enter:
if "%IP%"=="" (
    echo Nisi upisao IP adresu.
    pause
    exit /b 1
)
echo.
echo Povezujem se na %IP%:7000 ... host mora da odobri vezu.
echo.
if not exist "%~dp0remote-desk.exe" (
    echo GRESKA: nema remote-desk.exe u ovom folderu: %~dp0
    echo Kopiraj sva cetiri fajla iz dist foldera zajedno.
    pause
    exit /b 1
)
"%~dp0remote-desk.exe" view %IP%:7000
echo.
echo Viewer se zavrsio (vidi poruku iznad ako je bilo greske).
pause
