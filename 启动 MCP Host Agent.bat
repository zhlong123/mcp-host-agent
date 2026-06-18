@echo off
set "EXE=%~dp0target\release\mcp-host-agent-app.exe"
if not exist "%EXE%" (
  echo 未找到 release 版本，请先运行: npm run build:app
  pause
  exit /b 1
)
start "" "%EXE%"
