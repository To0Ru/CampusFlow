@echo off
chcp 65001 >nul
cd /d "%~dp0"

echo ============================================
echo   CampusFlow Web 控制台
echo ============================================
echo.

where python >nul 2>nul
if errorlevel 1 (
    echo [!!] 找不到 python，请先安装 Python 3.8+ 并加入 PATH
    pause
    exit /b 1
)

python campusflow.py web --watch %*

echo.
echo 服务已退出。
pause
