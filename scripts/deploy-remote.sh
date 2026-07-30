#!/usr/bin/env bash
set -eo pipefail

NEW_BINARY="/tmp/amll-ttml-api-new"
TARGET_BINARY="/usr/local/bin/amll-ttml-api"
UPSTREAM_CONF="/etc/nginx/conf.d/amll_upstream.conf"

echo "[i] 开始部署 AMLL TTML API"

if [ ! -f "$NEW_BINARY" ]; then
  echo "[!] 找不到新版本的二进制文件 $NEW_BINARY"
  exit 1
fi

chmod +x "$NEW_BINARY"

CURRENT_PORT=""
if systemctl is-active --quiet amll-ttml-api@3000.service; then
  CURRENT_PORT=3000
  TARGET_PORT=3001
elif systemctl is-active --quiet amll-ttml-api@3001.service; then
  CURRENT_PORT=3001
  TARGET_PORT=3000
else
  CURRENT_PORT=3000
  TARGET_PORT=3001
fi

echo "[i] 当前活动端口: ${CURRENT_PORT:-无} -> 目标部署端口: ${TARGET_PORT}"

NEW_SERVICE_FILE="/tmp/amll-ttml-api@.service"
TARGET_SERVICE_FILE="/etc/systemd/system/amll-ttml-api@.service"

if [ -f "$NEW_SERVICE_FILE" ]; then
  echo "[i] 更新 Systemd 服务文件 $TARGET_SERVICE_FILE"
  sudo mv "$NEW_SERVICE_FILE" "$TARGET_SERVICE_FILE"
  sudo systemctl daemon-reload
fi

sudo mv "$NEW_BINARY" "$TARGET_BINARY"
sudo chmod +x "$TARGET_BINARY"

echo "[i] 正在启动 amll-ttml-api@${TARGET_PORT}.service"
sudo systemctl restart "amll-ttml-api@${TARGET_PORT}.service"

echo "[i] 正在进行健康检查 (http://127.0.0.1:${TARGET_PORT}/api/v1/status)"
HEALTHY=false
for i in {1..15}; do
  RESPONSE=$(curl -s "http://127.0.0.1:${TARGET_PORT}/api/v1/status" || true)
  if echo "$RESPONSE" | grep -q '"status":"ok"'; then
    HEALTHY=true
    echo "[i] 健康检查成功 (${i}s), 响应: ${RESPONSE}"
    break
  fi
  sleep 1
done

if [ "$HEALTHY" = false ]; then
  echo "[!] 新版本服务在端口 ${TARGET_PORT} 上启动失败/健康检查超时"
  echo "[!] 正在尝试停止故障实例并保持旧版本运行"
  sudo systemctl stop "amll-ttml-api@${TARGET_PORT}.service" || true
  exit 1
fi

echo "[i] 更新 Nginx upstream 配置指向端口 ${TARGET_PORT}"
sudo bash -c "cat <<EOF > $UPSTREAM_CONF
upstream amll_api_backend {
    server 127.0.0.1:${TARGET_PORT};
}
EOF"

echo "[i] 重载 Nginx..."
sudo nginx -t
sudo nginx -s reload

if [ -n "$CURRENT_PORT" ] && [ "$CURRENT_PORT" != "$TARGET_PORT" ]; then
  echo "[i] 正在停止旧服务 amll-ttml-api@${CURRENT_PORT}.service"
  sleep 3
  sudo systemctl stop "amll-ttml-api@${CURRENT_PORT}.service" || true
fi

if systemctl is-active --quiet amll-ttml-api.service; then
  echo "[i] 停止旧版本服务 amll-ttml-api.service ..."
  sudo systemctl stop amll-ttml-api.service || true
  sudo systemctl disable amll-ttml-api.service || true
fi

echo "[i] AMLL TTML API 更新成功, 端口: ${TARGET_PORT}"
