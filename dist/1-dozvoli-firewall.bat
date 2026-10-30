@echo off
rem Pokreni DESNIM KLIKOM -> "Run as administrator", samo na HOST racunaru, samo jednom.
net session >nul 2>&1
if errorlevel 1 (
    echo.
    echo GRESKA: ovaj fajl mora da se pokrene kao administrator.
    echo Zatvori ovaj prozor, klikni DESNIM tasterom na fajl i izaberi "Run as administrator".
    echo.
    pause
    exit /b 1
)
cd /d "%~dp0"
netsh advfirewall firewall delete rule name="Remote Desk" >nul 2>&1
netsh advfirewall firewall add rule name="Remote Desk" dir=in action=allow protocol=UDP localport=7000 program="%~dp0remote-desk.exe" profile=any
echo.
echo Gotovo. Firewall sada dozvoljava dolazni UDP na portu 7000 za ovaj program.
echo Sledeci korak: dvoklik na 2-pokreni-host.bat
echo.
pause
