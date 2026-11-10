@echo off
chcp 65001 >nul
cd /d "%~dp0"
py -3 --version >nul 2>&1
if errorlevel 1 (
  echo 请从 python.org 安装 Python 3.9 或更新版本，或下载云编译版。
  pause
  exit /b 1
)
py -3 run.py
if errorlevel 1 pause
