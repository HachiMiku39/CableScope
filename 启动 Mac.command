#!/bin/zsh
cd "${0:A:h}" || exit 1
if ! command -v python3 >/dev/null; then
  print '请先从 python.org 安装 Python 3.9 或更新版本，或下载云编译版。'
  read '?按回车关闭'
  exit 1
fi
python3 run.py
