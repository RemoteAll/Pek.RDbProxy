#!/usr/bin/env bash
# Pek.RDbProxy 一键安装（Linux）
#
#   ./install.sh [程序文件] [安装目录]
#   ./install.sh ./pek-rdbproxy /opt/PekRDbProxy
#
# 步骤：停旧服务 → 复制程序 → 首次生成配置 → 注册 systemd 服务 → 启动 → 可选注册星尘
set -euo pipefail

BIN="${1:-./pek-rdbproxy}"
TARGET="${2:-/opt/PekRDbProxy}"
EXE="pek-rdbproxy"

if [ ! -f "$BIN" ]; then
  echo "找不到程序：$BIN" >&2
  echo "用法：./install.sh <程序文件> [安装目录]" >&2
  exit 1
fi

echo "== Pek.RDbProxy 安装 =="
echo "  源程序：$BIN"
echo "  安装到：$TARGET"

SUDO=""
if [ "$(id -u)" -ne 0 ]; then SUDO="sudo"; fi

# 1) 停旧服务
if command -v systemctl >/dev/null 2>&1 && systemctl list-unit-files 2>/dev/null | grep -q '^StarAgentRust\|^PekRDbProxy'; then
  echo "  停止旧服务…"
  $SUDO systemctl stop PekRDbProxy 2>/dev/null || true
fi
pkill -f "$EXE" 2>/dev/null || true
sleep 1

# 2) 复制程序
$SUDO mkdir -p "$TARGET"
echo "  复制程序…"
$SUDO cp -f "$BIN" "$TARGET/$EXE"
$SUDO chmod +x "$TARGET/$EXE"

# 3) 首次生成配置（不覆盖已有配置）
CONFIG="$TARGET/Config/Proxy.toml"
if [ ! -f "$CONFIG" ]; then
  echo "  生成配置模板…"
  ( cd "$TARGET" && $SUDO ./"$EXE" -init >/dev/null )
else
  echo "  保留已有配置：$CONFIG"
fi

# 4) 注册 systemd 服务并启动
echo "  注册 systemd 服务…"
( cd "$TARGET" && $SUDO ./"$EXE" -install >/dev/null )
echo "  启动服务…"
( cd "$TARGET" && $SUDO ./"$EXE" -start >/dev/null ) || true
sleep 2
if command -v systemctl >/dev/null 2>&1; then
  systemctl --no-pager --lines=0 status PekRDbProxy 2>/dev/null | head -3 || true
fi

# 5) 注册到星尘（有 StarAgent 就自动注册；没有则跳过）
AGENT_CFG=""
for p in /www/Agent/Config/StarAgent.config /www/Stardust/Agent/Config/StarAgent.config /opt/StarAgent/Config/StarAgent.config; do
  if [ -f "$p" ]; then AGENT_CFG="$p"; break; fi
done
if [ -n "$AGENT_CFG" ]; then
  echo "  检测到星尘：$AGENT_CFG，注册托管…"
  ( cd "$TARGET" && $SUDO ./"$EXE" -RegisterAgent ) || true
else
  echo "  未检测到星尘（可稍后运行：$TARGET/$EXE -RegisterAgent）"
fi

cat <<EOF

安装完成。
  安装目录：$TARGET
  配置文件：$CONFIG（连接串已用 Config/.key 加密，勿泄露该文件）
  面板地址：http://127.0.0.1:5599/  （首次账号 admin/admin，登录后请改密）

后续操作：
  查看状态：$TARGET/$EXE -status
  停止/启动：$TARGET/$EXE -stop / -start
  注册星尘：$TARGET/$EXE -RegisterAgent
  升级：把新程序放进 $TARGET/Update/（10 秒内自动替换并重启）

远程访问面板（二选一）：
  1) SSH 隧道：ssh -L 5599:127.0.0.1:5599 user@server
  2) 面板「设置」打开「允许远程访问」并把鉴权级别设为 Full（务必先改密码）
EOF
