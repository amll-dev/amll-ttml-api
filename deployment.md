# AMLL TTML API 服务器部署与运维指南

本指南详细说明了如何在 Ubuntu/Debian Linux 服务器上从零搭建、配置与运维 **AMLL TTML API** 服务。部署架构基于 **Docker 容器化蓝绿无缝热部署**、**Nginx 反向代理 + Let's Encrypt HTTPS** 以及 **GitHub Actions + GitHub Packages (GHCR) 自动化 CI/CD**。

---

## 1. 部署架构概览

```text
                        ┌──────────────────────────────────────────────────────────┐
                        │                 域名: https://api.amll.dev                │
                        └────────────────────────────┬─────────────────────────────┘
                                                     │ (HTTPS 443)
                                                     ▼
                        ┌──────────────────────────────────────────────────────────┐
                        │                     Nginx 反向代理                       │
                        └────────────────────────────┬─────────────────────────────┘
                                                     │ (http://127.0.0.1:3000 或 3001)
                                                     ▼
                        ┌──────────────────────────────────────────────────────────┐
                        │         Docker 蓝绿容器 (GHCR 镜像)                       │
                        │ ┌──────────────────────────┐ ┌──────────────────────────┐ │
                        │ │ amll-api-blue (@3000)    │ │ amll-api-green (@3001)   │ │
                        │ └─────────────┬────────────┘ └─────────────┬────────────┘ │
                        └───────────────┼────────────────────────────┼──────────────┘
                                        │ (Volume /data, /var/lib/..)│
                                        ▼                            ▼
                        ┌──────────────────────────────────────────────────────────┐
                        │   环境变量配置文件 (服务器保密): /etc/amll-ttml-api/env  │
                        │   包含: SENTRY_DSN, SYNC_SECRET, DATABASE_URL,           │
                        │         ANALYTICS_DIR, ANALYTICS_IP_KEY, ANALYTICS_SECRET│
                        └──────────────────────────────────────────────────────────┘
```

* **无缝热更新 (Zero-Downtime)**：利用双容器插槽（`amll-api-blue` 端口 3000 与 `amll-api-green` 端口 3001）平滑交替轮换，配合健康检查与 Nginx 重载，实现发布更新时外部请求完全零感停机。
* **配置与代码解耦**：所有敏感密钥（`SENTRY_DSN`、`SYNC_SECRET`、`ANALYTICS_IP_KEY`、`ANALYTICS_SECRET`）独立保存在服务器本地 `/etc/amll-ttml-api/env`，绝不提交至代码仓库。

---

## 2. 首次服务器初始化配置

### 2.1 基础依赖安装
在全新的 Ubuntu / Debian 服务器上安装必需的基础系统工具：

```bash
sudo apt-get update && sudo apt-get install -y \
    ca-certificates \
    curl \
    git \
    nginx \
    libnginx-mod-http-brotli-filter \
    libnginx-mod-http-brotli-static \
    certbot \
    python3-certbot-nginx
```

### 2.2 创建环境变量配置文件
在服务器上创建安全配置目录与环境文件：

```bash
# 1. 创建配置目录
sudo mkdir -p /etc/amll-ttml-api

# 2. 写入敏感环境变量配置
sudo bash -c 'cat <<EOF > /etc/amll-ttml-api/env
PORT=3000
SENTRY_DSN=填入你的 Sentry DSN 链接
SYNC_SECRET=/webhook/sync 接口用的密钥
ANALYTICS_DIR=/var/lib/amll-ttml-api/analytics
ANALYTICS_IP_KEY=客户端 IP 的 HMAC 密钥，用 openssl rand -hex 32 生成
ANALYTICS_SECRET=团队下载统计文件用的 token，用 openssl rand -hex 32 生成
EOF'

# 3. 严格保护文件权限 (仅 root / sudo 可读)
sudo chmod 600 /etc/amll-ttml-api/env

# 4. 验证文件内容
sudo cat /etc/amll-ttml-api/env
```

### 2.3 内核连接跟踪与网络参数调优
为避免高峰期流量导致 Linux 内核连接跟踪表（`nf_conntrack`）满载丢包（导致反代 499 / 504 错误），配置内核参数：

```bash
sudo bash -c 'cat <<EOF > /etc/sysctl.d/99-conntrack.conf
net.netfilter.nf_conntrack_max = 65536
net.netfilter.nf_conntrack_tcp_timeout_established = 1800
net.netfilter.nf_conntrack_tcp_timeout_time_wait = 30
EOF'

# 即时生效
sudo sysctl -p /etc/sysctl.d/99-conntrack.conf
```

---

## 3. Systemd 服务模版配置

创建 Systemd 实例化服务模版 `/etc/systemd/system/amll-ttml-api@.service`：

```bash
sudo bash -c 'cat <<EOF > /etc/systemd/system/amll-ttml-api@.service
[Unit]
Description=AMLL TTML API 服务 (Port %i)
After=network.target

[Service]
Type=simple
Environment=PORT=%i
EnvironmentFile=-/etc/amll-ttml-api/env
ExecStart=/usr/local/bin/amll-ttml-api
Restart=always
RestartSec=3

SyslogIdentifier=amll-ttml-api-%i

[Install]
WantedBy=multi-user.target
EOF'

# 重载 Systemd 守护进程
sudo systemctl daemon-reload
```

---

## 4. Nginx 与 SSL 证书配置

### 4.1 创建初始 Upstream 配置文件
创建 `/etc/nginx/conf.d/amll_upstream.conf`：

```bash
sudo bash -c 'cat <<EOF > /etc/nginx/conf.d/amll_upstream.conf
upstream amll_api_backend {
    server 127.0.0.1:3000;
    keepalive 32;
}
EOF'
```

### 4.2 创建 Nginx 站点配置
创建 `/etc/nginx/sites-available/amll-ttml-api` 并软链接至 `sites-enabled`：

```bash
sudo bash -c 'cat <<EOF > /etc/nginx/sites-available/amll-ttml-api
server {
    listen 80;
    server_name api.amll.dev;
    
    return 301 https://\$host\$request_uri;
}

server {
    listen 443 ssl http2;
    server_name api.amll.dev;

    ssl_certificate /etc/letsencrypt/live/api.amll.dev/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.amll.dev/privkey.pem;

    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers on;

    ssl_session_cache   shared:SSL:10m;
    ssl_session_timeout 1d;
    ssl_early_data on;
    keepalive_timeout   300s;

    error_log  /var/log/nginx/amll-api-error.log;
    access_log /var/log/nginx/amll-api-access.log;

    limit_req_status 429;

    add_header X-Content-Type-Options "nosniff" always;
    add_header Strict-Transport-Security "max-age=31536000" always;
    add_header Content-Security-Policy "default-src '\''none'\''; frame-ancestors '\''none'\''; sandbox" always;

    gzip on;
    gzip_vary on;
    gzip_proxied any;
    gzip_comp_level 9;
    gzip_min_length 256;
    gzip_http_version 1.1;
    gzip_types
        application/json
        text/plain
        text/css
        application/javascript
        text/javascript
        application/xml
        text/xml
        application/xml+rss
        image/svg+xml;

    brotli on;
    brotli_comp_level 6;
    brotli_min_length 256;
    brotli_types
        application/json
        text/plain
        text/css
        application/javascript
        text/javascript
        application/xml
        text/xml
        application/xml+rss
        image/svg+xml;

    location / {
        proxy_pass http://amll_api_backend;
        proxy_http_version 1.1;
        proxy_set_header Connection "";

        limit_req zone=apilimit burst=50 nodelay;

        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_set_header Early-Data \$ssl_early_data;
    }
}
EOF'

# 启用站点配置
sudo ln -sf /etc/nginx/sites-available/amll-ttml-api /etc/nginx/sites-enabled/
```

### 4.3 申请 HTTPS 证书与重载 Nginx

```bash
# 申请 Let's Encrypt SSL 证书
sudo certbot --nginx -d api.amll.dev -m your_email@domain.com --agree-tos --non-interactive

# 校验并重载 Nginx
sudo nginx -t && sudo nginx -s reload
```

---

## 5. GitHub Actions 自动化 CI/CD 配置

### 5.1 设置 GitHub Secrets
在 GitHub 仓库中添加以下 Secrets：

| Secret 名称      | 示例/说明                                                       |
| ---------------- | --------------------------------------------------------------- |
| `SERVER_HOST`    | 服务器公网 IP 地址（例如 `20.198.86.114`）                      |
| `SERVER_USER`    | SSH 登录用户名（例如 `azureuser`）                              |
| `SERVER_SSH_KEY` | SSH 私钥内容（格式为 `-----BEGIN OPENSSH PRIVATE KEY-----...`） |

### 5.2 自动化部署触发
当需要发布新版本时：
1. 打开 GitHub 仓库页面，进入 `Actions` 选项卡。
2. 选择 **Build and Deploy** 工作流。
3. 点击 **Run workflow** 手动触发构建与无缝部署（可输入指定 Tag 或留空使用默认的 Commit SHA 与 latest）。

自动化脚本流会自动完成：
- 通过 Docker Buildx 与 `cargo-chef` 缓存构建生产容器镜像，并推送到 GitHub Container Registry (`ghcr.io/amll-dev/amll-ttml-api`)。
- 通过 SSH 将部署脚本 `scripts/deploy-docker.sh` 传输到服务器。
- 在服务器上拉取新镜像，并根据当前运行状态启动对侧备用容器（3000 端口 `amll-api-blue` 或 3001 端口 `amll-api-green`）。
- 执行容器健康探活检查 (`/api/v1/status`)。
- 探活成功后原子覆盖 Nginx upstream 配置，平滑重载 Nginx。
- 向旧容器发送优雅下线信号（等待在途请求处理与分析数据刷盘），最后清理废弃容器与悬空镜像。

---

## 6. 日常运维与故障排查指令

### 6.1 查看服务运行状态
```bash
# 查看当前正在运行的容器与资源占用
docker ps -f "name=amll-api"
docker stats --no-stream

# 查看当前 Nginx 指向的后端端口
cat /etc/nginx/conf.d/amll_upstream.conf
```

### 6.2 实时日志查看
```bash
# 实时跟踪活动容器的日志
docker logs -f --tail 100 amll-api-blue
# 或若是 green 槽位:
docker logs -f --tail 100 amll-api-green

# 查看 Nginx 访问与错误日志
sudo tail -f /var/log/nginx/amll-api-error.log
```

### 6.3 更新环境变量配置
如果修改了 `/etc/amll-ttml-api/env`（例如更新了 `SENTRY_DSN` 或 `SYNC_SECRET`）：

```bash
# 1. 编辑环境变量文件
sudo nano /etc/amll-ttml-api/env

# 2. 重启当前活动容器使配置生效
docker restart amll-api-blue
# 或若当前是 green 槽位:
docker restart amll-api-green
```

### 6.4 快速故障回滚
如果新发布的版本存在非预期缺陷，可通过指定历史稳定的镜像 Tag/Commit SHA 快速秒级回滚：

```bash
# 在服务器上手动执行回滚至指定镜像版本 (例如 sha-44cf460)
bash /tmp/deploy-docker.sh sha-44cf460
```

### 6.5 请求统计
设置了 `ANALYTICS_DIR` 后，服务会把每个 HTTP 请求先记录到 `$ANALYTICS_DIR/buffer.db`（SQLite，WAL 模式，目录不存在时自动创建）；每个北京时间自然日收齐后（0 点 10 分之后），自动转成 `$ANALYTICS_DIR/parquet/YYYY-MM-DD.parquet` 并从缓冲库删掉。蓝绿两个实例共用同一个目录，同一天只会被转换一次。

* **客户端标识**：只存 `X-Real-IP` 的 HMAC 摘要，不存原始 IP。需要封禁时用 Nginx access log 里的原始 IP。未设置 `ANALYTICS_IP_KEY` 时照常记录，只是客户端标识留空，并在启动时报错上报 Sentry；更换密钥等同于独立客户端重新开始计数。
* **团队下载**：团队用 `ANALYTICS_SECRET` 作为 token，经 `/v1/admin/analytics/files` 下载每日文件，用法见 [`docs/analytics.md`](docs/analytics.md)。未设置时下载端点返回 500；token 泄露时换一个新值并重启服务即可作废旧的。
* **保留策略**：每日文件在服务器上保留 90 天；统计目录总大小超过 8 GiB 时从最旧的文件开始提前删除（日志 `Analytics directory over its size cap`）。长期存档靠团队每月把上个月的文件同步到网盘。
* **磁盘水位**：磁盘剩余空间低于 3 GiB 时暂停写入，日志出现 `Free disk space below the analytics watermark`，空间恢复后自动继续。
* **资源占用**：按 200 万请求 / 天估算，每日文件约 50 MB；转换时内存峰值约增加 70 MiB，持续一两分钟。缓冲库删除旧行后文件大小不会缩小，空闲页会被新数据复用。
* **故障隔离**：统计目录不可写或缓冲库打不开时只关闭统计，服务照常启动。
* **已知缺口**：Nginx `limit_req` 直接返回的 429 到不了应用，不会进统计。

```bash
# 查看每日文件与转换记录
sudo ls -lh /var/lib/amll-ttml-api/analytics/parquet/
sudo python3 -c "import sqlite3; db = sqlite3.connect('/var/lib/amll-ttml-api/analytics/buffer.db'); print(db.execute('SELECT count(*) FROM requests').fetchone()); [print(r) for r in db.execute('SELECT day, state, rows, bytes FROM conversions ORDER BY day DESC LIMIT 10')]"
```
