@echo off
setlocal
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0diagnose-windows-cli.ps1" -OutputPath "%~dp0pisper-cli-report.json"
set "diagnosticExitCode=%errorlevel%"
if "%diagnosticExitCode%"=="0" echo Report saved next to this script: pisper-cli-report.json
pause
exit /b %diagnosticExitCode%
