@echo off
title Remote Desk - HOST (deli ovaj ekran)
cd /d "%~dp0"
echo ============================================================
echo  REMOTE DESK - HOST
echo ============================================================
echo.
echo IP adresa ovog racunara (nju upisujes na viewer racunaru):
for /f "tokens=2 delims=:" %%a in ('ipconfig ^| findstr /c:"IPv4"') do echo    %%a
echo.
echo Kad se na viewer-u povezes, ovde ce se pojaviti pitanje:
echo    c + Enter = dozvoli gledanje I kontrolu misa/tastature
echo    v + Enter = samo gledanje
echo    bilo sta drugo = odbij
echo.
echo Hitni prekid: Ctrl+Alt+Shift+F12. Zatvaranje ovog prozora gasi host.
echo ------------------------------------------------------------
echo.
if not exist "%~dp0remote-desk.exe" (
    echo GRESKA: nema remote-desk.exe u ovom folderu: %~dp0
    echo Kopiraj sva cetiri fajla iz dist foldera zajedno.
    pause
    exit /b 1
)
"%~dp0remote-desk.exe" host
echo.
echo Host se zavrsio (vidi poruku iznad ako je bilo greske).
pause
