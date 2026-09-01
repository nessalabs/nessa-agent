@echo off
REM Windows entry for the launch host. Not yet run on a Windows box.
node "%~dp0scripts\launch\cli.mjs" %*
exit /b %ERRORLEVEL%
