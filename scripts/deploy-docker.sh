#!/usr/bin/env bash
set -eo pipefail

IMAGE_TAG="${1:-latest}"
REGISTRY="ghcr.io/amll-dev/amll-ttml-api"
FULL_IMAGE="${REGISTRY}:${IMAGE_TAG}"
UPSTREAM_CONF="/etc/nginx/conf.d/amll_upstream.conf"

echo "=========================================="
echo "[i] 正在使用容器部署 AMLL TTML API"
echo "[i] 目标镜像: ${FULL_IMAGE}"
echo "=========================================="

# 1. 预拉取最新容器镜像
echo "[1/6] 正在拉取容器镜像..."
docker pull "$FULL_IMAGE"

# 2. 判断当前活动槽位与目标槽位
CURRENT_SLOT=""
if docker ps --format '{{.Names}}' | grep -q "^amll-api-blue$"; then
  CURRENT_SLOT="blue"
  TARGET_SLOT="green"
  TARGET_PORT=3001
elif docker ps --format '{{.Names}}' | grep -q "^amll-api-green$"; then
  CURRENT_SLOT="green"
  TARGET_SLOT="blue"
  TARGET_PORT=3000
else
  # 首次从 Systemd 割接或无容器运行场景
  if grep -q "3000" "$UPSTREAM_CONF" 2>/dev/null || systemctl is-active --quiet amll-ttml-api@3000.service 2>/dev/null; then
    echo "[i] 检测到当前流量由 3000 端口承接，选择 green 槽位 (3001 端口) 作为首发目标"
    TARGET_SLOT="green"
    TARGET_PORT=3001
  else
    TARGET_SLOT="blue"
    TARGET_PORT=3000
  fi
fi

echo "[2/6] 槽位决策: 当前活动=[${CURRENT_SLOT:-Systemd/无}] -> 部署目标=[${TARGET_SLOT}] (端口 ${TARGET_PORT})"

# 清理可能残留的同名旧容器
docker rm -f "amll-api-${TARGET_SLOT}" 2>/dev/null || true

# 3. 启动新容器（严格限制内存与日志，挂载持久化卷与环境变量）
echo "[3/6] 正在启动目标容器 amll-api-${TARGET_SLOT}..."
docker run -d \
  --name "amll-api-${TARGET_SLOT}" \
  --restart unless-stopped \
  --memory=280m --memory-swap=512m \
  --log-opt max-size=50m --log-opt max-file=3 \
  -p 127.0.0.1:${TARGET_PORT}:3000 \
  --env-file /etc/amll-ttml-api/env \
  -e PORT=3000 \
  -v /data:/data \
  -v /var/lib/amll-ttml-api/analytics:/var/lib/amll-ttml-api/analytics \
  "$FULL_IMAGE"

# 4. 健康检查探活（等待 SQLite 索引重建完成）
echo "[4/6] 正在对新容器进行健康检查 (http://127.0.0.1:${TARGET_PORT}/api/v1/status)..."
HEALTHY=false
for i in {1..20}; do
  RESPONSE=$(curl -s "http://127.0.0.1:${TARGET_PORT}/api/v1/status" || true)
  if echo "$RESPONSE" | grep -q '"status":"ok"'; then
    HEALTHY=true
    echo "[i] 新容器就绪 (耗时 ${i}s): ${RESPONSE}"
    break
  fi
  sleep 1
done

if [ "$HEALTHY" = false ]; then
  echo "[!] 错误: 新容器在端口 ${TARGET_PORT} 上启动失败或健康检查超时！"
  echo "[!] 正在自动回滚: 销毁故障容器，保持当前线上服务稳定运行"
  docker stop "amll-api-${TARGET_SLOT}" || true
  docker rm "amll-api-${TARGET_SLOT}" || true
  exit 1
fi

# 5. 原子切换 Nginx Upstream
echo "[5/6] 健康检查通过，原子切换 Nginx Upstream 至端口 ${TARGET_PORT}..."
sudo bash -c "cat <<EOF > $UPSTREAM_CONF
upstream amll_api_backend {
    server 127.0.0.1:${TARGET_PORT};
}
EOF"

sudo nginx -t
sudo nginx -s reload
echo "[i] Nginx upstream 已无缝切换至 ${TARGET_PORT}"

# 6. 优雅下线并清理旧服务
echo "[6/6] 优雅下线并清理旧实例..."
if [ -n "$CURRENT_SLOT" ]; then
  echo "[i] 停止旧容器 amll-api-${CURRENT_SLOT} (等待在途请求处理与分析数据刷盘)..."
  docker stop -t 5 "amll-api-${CURRENT_SLOT}" || true
  docker rm "amll-api-${CURRENT_SLOT}" || true
fi

# 若存在旧版 Systemd 服务，自动停止并禁用
if systemctl is-active --quiet amll-ttml-api@3000.service 2>/dev/null; then
  echo "[i] 停用旧版 Systemd 实例 amll-ttml-api@3000.service..."
  sudo systemctl stop amll-ttml-api@3000.service || true
  sudo systemctl disable amll-ttml-api@3000.service || true
fi
if systemctl is-active --quiet amll-ttml-api@3001.service 2>/dev/null; then
  echo "[i] 停用旧版 Systemd 实例 amll-ttml-api@3001.service..."
  sudo systemctl stop amll-ttml-api@3001.service || true
  sudo systemctl disable amll-ttml-api@3001.service || true
fi

# 修剪废弃悬空镜像，防止磁盘膨胀
echo "[i] 清理悬空 Docker 镜像..."
docker image prune -f

echo "=========================================="
echo "[✓] 部署成功完成！"
echo "[✓] 当前运行容器: amll-api-${TARGET_SLOT} (端口 ${TARGET_PORT})"
echo "=========================================="
