@echo off
cd /d "%~dp0"
"%~dp0AudioProbe.exe" --operator
if errorlevel 1 echo Test did not finish. No automatic retry or upload.
pause
