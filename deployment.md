# AMLL TTML API 服务器部署与运维指南

本指南详细说明了如何在 Ubuntu/Debian Linux 服务器上从零搭建、配置与运维 **AMLL TTML API** 服务。部署架构基于 **Systemd 蓝绿无缝热部署**、**Nginx 反向代理 + Let's Encrypt HTTPS** 以及 **GitHub Actions 自动化 CI/CD**。

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
                        │        Systemd 蓝绿服务 (amll-ttml-api@.service)          │
                        │ ┌──────────────────────────┐ ┌──────────────────────────┐ │
                        │ │  实例 @3000 (活动/备用)  │ │  实例 @3001 (活动/备用)  │ │
                        │ └─────────────┬────────────┘ └─────────────┬────────────┘ │
                        └───────────────┼────────────────────────────┼──────────────┘
                                        │                            │
                                        ▼                            ▼
                        ┌──────────────────────────────────────────────────────────┐
                        │   环境变量配置文件 (服务器保密): /etc/amll-ttml-api/env  │
                        │   包含: SENTRY_DSN, SYNC_SECRET, DATABASE_URL            │
                        └──────────────────────────────────────────────────────────┘
```

* **无缝热更新 (Zero-Downtime)**：利用 Systemd 模板服务 (`amll-ttml-api@.service`) 在端口 3000 与 3001 之间平滑热切换，配合健康检查与 Nginx 重载，实现发布更新时用户完全零感停机。
* **配置与代码解耦**：所有敏感密钥（`SENTRY_DSN`、`SYNC_SECRET`）独立保存在服务器本地 `/etc/amll-ttml-api/env`，绝不提交至代码仓库。

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
EOF'

# 3. 严格保护文件权限 (仅 root / sudo 可读)
sudo chmod 600 /etc/amll-ttml-api/env

# 4. 验证文件内容
sudo cat /etc/amll-ttml-api/env
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

        limit_req zone=apilimit burst=50 nodelay;

        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
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
3. 点击 **Run workflow** 手动触发构建与无缝部署。

自动化脚本流会自动完成：
- 交叉编译 Linux 64位二进制。
- `scp` 传输新版可执行文件与部署脚本到服务器。
- 启动目标端口实例、执行自动健康检查 (`/v1/status`)。
- 健康检查成功后自动重载 Nginx upstream 指向新端口，最后优雅停止旧端口服务。

---

## 6. 日常运维与故障排查指令

### 6.1 查看服务运行状态
```bash
# 查看 3000 或 3001 端口服务状态
sudo systemctl status amll-ttml-api@3000.service
sudo systemctl status amll-ttml-api@3001.service

# 查看活动 Nginx 指向的后端端口
cat /etc/nginx/conf.d/amll_upstream.conf
```

### 6.2 实时日志查看
```bash
# 查看最近 100 行 API 实时日志
sudo journalctl -u amll-ttml-api@3000.service -f -n 100
sudo journalctl -u amll-ttml-api@3001.service -f -n 100

# 查看 Nginx 错误日志
sudo tail -f /var/log/nginx/amll-api-error.log
```

### 6.3 更新环境变量配置
如果修改了 `/etc/amll-ttml-api/env`（例如更新了 `SENTRY_DSN` 或 `SYNC_SECRET`）：

```bash
# 1. 编辑环境变量文件
sudo nano /etc/amll-ttml-api/env

# 2. 重启服务使新配置生效
sudo systemctl restart amll-ttml-api@3000.service
# 或重启另一个端口服务
sudo systemctl restart amll-ttml-api@3001.service
```
