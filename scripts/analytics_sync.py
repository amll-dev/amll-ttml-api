#!/usr/bin/env python3
"""把请求统计的每日 Parquet 文件同步到本地目录。

只用 Python 标准库。流程：
1. 从 /v1/admin/analytics/files 取文件清单（需要 ANALYTICS_SECRET 对应的 token）
2. 本地已有且大小一致的文件跳过（每日文件一经发布不再改动）
3. 其余文件下载到 `<name>.part`，校验 sha256 后改名到位；中断的下载下次运行时用 Range 续传

用法：
    AMLL_ANALYTICS_TOKEN=<token> python scripts/analytics_sync.py --dest ./amll-analytics

详见 docs/analytics.md。
"""

import argparse
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request

DEFAULT_BASE_URL = "https://api.amll.dev"
# 统计里按这个 UA 识别同步流量，归类表 clients.csv 里应有对应的规则，见 docs/analytics.md
USER_AGENT = "amll-analytics-sync/1.0"
CHUNK_BYTES = 1 << 20
TIMEOUT_SECONDS = 60


def open_url(url, token, extra_headers=None):
    headers = {"Authorization": f"Bearer {token}", "User-Agent": USER_AGENT}
    headers.update(extra_headers or {})
    return urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=TIMEOUT_SECONDS)


def fetch_manifest(base_url, token):
    with open_url(f"{base_url}/v1/admin/analytics/files", token) as resp:
        return json.load(resp)["data"]["files"]


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(CHUNK_BYTES), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(base_url, token, item, dest):
    final_path = os.path.join(dest, item["name"])
    part_path = final_path + ".part"

    offset = os.path.getsize(part_path) if os.path.exists(part_path) else 0
    if offset > item["bytes"]:
        os.remove(part_path)
        offset = 0

    if offset < item["bytes"]:
        headers = {"Range": f"bytes={offset}-"} if offset else None
        with open_url(f"{base_url}/v1/admin/analytics/files/{item['name']}", token, headers) as resp:
            # 服务端没按 Range 返回 206 时只能从头下载
            if offset and resp.status != 206:
                offset = 0
            with open(part_path, "ab" if offset else "wb") as f:
                for chunk in iter(lambda: resp.read(CHUNK_BYTES), b""):
                    f.write(chunk)

    actual = sha256_of(part_path)
    if actual != item["sha256"]:
        os.remove(part_path)
        raise RuntimeError(f"sha256 不一致（期望 {item['sha256']}，实际 {actual}），已删除，下次重新下载")

    os.replace(part_path, final_path)


def main():
    parser = argparse.ArgumentParser(description="同步 AMLL TTML API 的请求统计每日 Parquet 文件")
    parser.add_argument("--dest", default="amll-analytics", help="本地目录，默认 ./amll-analytics")
    parser.add_argument(
        "--base-url",
        default=os.environ.get("AMLL_ANALYTICS_BASE_URL", DEFAULT_BASE_URL),
        help=f"API 地址，默认 {DEFAULT_BASE_URL}",
    )
    parser.add_argument(
        "--token",
        default=os.environ.get("AMLL_ANALYTICS_TOKEN"),
        help="下载 token，也可以用环境变量 AMLL_ANALYTICS_TOKEN（推荐，避免留在 shell 历史里）",
    )
    parser.add_argument("--since", metavar="YYYY-MM-DD", help="只同步这一天（含）之后的文件")
    parser.add_argument("--verify", action="store_true", help="对本地已有的文件也重新校验 sha256")
    args = parser.parse_args()

    if not args.token:
        parser.error("需要 --token 或环境变量 AMLL_ANALYTICS_TOKEN")

    base_url = args.base_url.rstrip("/")
    os.makedirs(args.dest, exist_ok=True)

    try:
        files = fetch_manifest(base_url, args.token)
    except urllib.error.HTTPError as e:
        hint = "，token 不对" if e.code == 401 else ""
        sys.exit(f"获取文件清单失败：HTTP {e.code}{hint}")
    except urllib.error.URLError as e:
        sys.exit(f"获取文件清单失败：{e.reason}")

    if args.since:
        files = [item for item in files if item["day"] >= args.since]

    downloaded = up_to_date = failed = 0
    for item in files:
        final_path = os.path.join(args.dest, item["name"])
        if (
            os.path.exists(final_path)
            and os.path.getsize(final_path) == item["bytes"]
            and (not args.verify or sha256_of(final_path) == item["sha256"])
        ):
            up_to_date += 1
            continue

        print(f"下载 {item['name']}（{item['bytes'] / 1e6:.1f} MB，{item['rows']} 行）", flush=True)
        try:
            download(base_url, args.token, item, args.dest)
            downloaded += 1
        except (urllib.error.URLError, OSError, RuntimeError) as e:
            print(f"  失败：{e}", file=sys.stderr)
            failed += 1

    print(f"完成：新下载 {downloaded} 个，已是最新 {up_to_date} 个，失败 {failed} 个。本次范围内共 {len(files)} 个文件。")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
