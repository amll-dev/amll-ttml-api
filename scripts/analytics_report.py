#!/usr/bin/env python3
"""AMLL TTML API 请求统计的本地分析工具。

读 ./amll-analytics/*.parquet（scripts/analytics_sync.py 的下载目录），
按北京时间自然日切分。默认分析最近一个有数据的日子。

用法：
    python scripts/analytics_report.py                    # 最近一天
    python scripts/analytics_report.py --date 2026-09-28  # 指定某天
    python scripts/analytics_report.py --since 2026-09-27 --until 2026-09-28
    python scripts/analytics_report.py --json             # 输出 JSON 供进一步处理
"""

import argparse
import glob
import json
import os
import sys

import duckdb

DATA_GLOB = "amll-analytics/*.parquet"


def connect():
    if not glob.glob(DATA_GLOB):
        sys.exit(f"没有找到数据文件（{DATA_GLOB}）。先运行 scripts/analytics_sync.py 同步。")
    con = duckdb.connect()
    con.execute("PRAGMA timezone='Asia/Shanghai'")
    con.execute(
        f"""
        CREATE VIEW requests AS
        SELECT *, timezone('Asia/Shanghai', ts) AS ts_local
        FROM read_parquet('{DATA_GLOB}', union_by_name = true)
        """
    )
    return con


def resolve_range(con, args):
    days = [r[0] for r in con.execute(
        "SELECT DISTINCT CAST(ts_local AS DATE) d FROM requests ORDER BY d"
    ).fetchall()]
    if not days:
        sys.exit("数据文件里没有请求记录。")
    if args.date:
        since = until = args.date
    elif args.since or args.until:
        since = args.since or days[0].isoformat()
        until = args.until or days[-1].isoformat()
    else:
        since = until = days[-1].isoformat()
    return since, until, days


def q(con, sql, params=None):
    return con.execute(sql, params or []).fetchall()


def section(title):
    print(f"\n{'=' * 68}\n{title}\n{'=' * 68}")


def report(con, since, until):
    where = "CAST(ts_local AS DATE) BETWEEN ? AND ?"
    p = [since, until]
    out = {"since": since, "until": until}

    section(f"概览  {since} ~ {until}")

    n, mn, mx, clients, uas = q(con, f"""
        SELECT count(*), min(ts_local), max(ts_local),
               count(DISTINCT client_id), count(DISTINCT user_agent)
        FROM requests WHERE {where}
    """, p)[0]
    if n == 0:
        sys.exit(f"{since} ~ {until} 没有请求记录。")
    span = (mx - mn).total_seconds()
    print(f"请求数        {n:,}")
    print(f"时间范围      {mn:%Y-%m-%d %H:%M:%S} ~ {mx:%Y-%m-%d %H:%M:%S}（北京时间）")
    print(f"覆盖时长      {span / 3600:.2f} 小时")
    print(f"平均 QPS      {n / span if span else 0:.2f}")
    print(f"独立客户端    {clients:,}")
    print(f"独立 UA       {uas:,}")
    out.update(requests=n, first=str(mn), last=str(mx), clients=clients)

    # 延迟
    section("延迟")
    p50, p90, p99, p999, mx_us, avg = q(con, f"""
        SELECT quantile_cont(latency_us, 0.5), quantile_cont(latency_us, 0.9),
               quantile_cont(latency_us, 0.99), quantile_cont(latency_us, 0.999),
               max(latency_us), avg(latency_us)
        FROM requests WHERE {where}
    """, p)[0]
    print(f"p50      {p50 / 1000:9.2f} ms")
    print(f"p90      {p90 / 1000:9.2f} ms")
    print(f"p99      {p99 / 1000:9.2f} ms")
    print(f"p99.9    {p999 / 1000:9.2f} ms")
    print(f"max      {mx_us / 1000:9.2f} ms")
    print(f"mean     {avg / 1000:9.2f} ms")
    out["latency_ms"] = dict(p50=p50 / 1000, p90=p90 / 1000, p99=p99 / 1000,
                             p999=p999 / 1000, max=mx_us / 1000)

    print("\n慢请求：")
    slow = {}
    for t in (10, 50, 100, 500, 1000):
        c = q(con, f"SELECT count(*) FROM requests WHERE {where} AND latency_us > ?",
              [*p, t * 1000])[0][0]
        slow[f">{t}ms"] = c
        print(f"  >{t:5d} ms  {c:7,d}  ({100 * c / n:.4f}%)")
    out["slow"] = slow

    # 状态码
    section("状态码")
    rows = q(con, f"""
        SELECT status, count(*) c FROM requests WHERE {where}
        GROUP BY ALL ORDER BY c DESC
    """, p)
    for s, c in rows:
        print(f"  {s}  {c:8,d}  {100 * c / n:6.2f}%")
    out["status"] = {str(s): c for s, c in rows}
    e5 = sum(c for s, c in rows if s >= 500)
    if e5:
        print(f"\n  !! 有 {e5:,} 条 5xx，需要排查")

    # 端点
    section("端点")
    for r, c in q(con, f"""
        SELECT coalesce(route, '(未匹配路由)') r, count(*) c
        FROM requests WHERE {where} GROUP BY ALL ORDER BY c DESC
    """, p):
        print(f"  {r:30s} {c:8,d}  {100 * c / n:6.2f}%")

    # 小时分布
    section("按小时请求量（北京时间）")
    hours = q(con, f"""
        SELECT EXTRACT(hour FROM ts_local)::INT h, count(*) c
        FROM requests WHERE {where} GROUP BY ALL ORDER BY h
    """, p)
    peak = max(c for _, c in hours) if hours else 1
    for h, c in hours:
        print(f"  {h:02d}:00  {c:7,d}  {'█' * round(c / peak * 40)}")
    if hours:
        hi = max(hours, key=lambda x: x[1])
        lo = min(hours, key=lambda x: x[1])
        print(f"\n  峰值 {hi[0]:02d}:00 {hi[1]:,} 条 / 谷值 {lo[0]:02d}:00 {lo[1]:,} 条"
              f" / 峰谷比 {hi[1] / lo[1]:.2f}x")
        out["peak_hour"] = {"hour": hi[0], "requests": hi[1]}
        out["trough_hour"] = {"hour": lo[0], "requests": lo[1]}

    # 客户端集中度
    section("客户端集中度")
    vc = q(con, f"""
        SELECT client_id, count(*) c FROM requests
        WHERE {where} AND client_id IS NOT NULL GROUP BY ALL ORDER BY c DESC
    """, p)
    if vc:
        total = sum(c for _, c in vc)
        for k in (1, 10, 100, 1000):
            if k <= len(vc):
                print(f"  top{k:<5d} 占比 {100 * sum(c for _, c in vc[:k]) / total:6.2f}%")
        print(f"  只发过 1 次请求的客户端  {sum(1 for _, c in vc if c == 1):,} 个")
    out["top1_pct"] = 100 * vc[0][1] / n if vc else 0

    # Top UA
    section("User-Agent top 10")
    for ua, c, cl in q(con, f"""
        SELECT user_agent, count(*) c, count(DISTINCT client_id) cl
        FROM requests WHERE {where} AND user_agent IS NOT NULL
        GROUP BY ALL ORDER BY c DESC LIMIT 10
    """, p):
        print(f"  {c:8,d} ({100 * c / n:5.2f}%)  {cl:5,d} 台  {ua[:52]}")

    # 慢请求样本
    section("最慢的 10 条")
    for ts, lat, st, rt, ua in q(con, f"""
        SELECT ts_local, latency_us, status, coalesce(route, raw_path), user_agent
        FROM requests WHERE {where} ORDER BY latency_us DESC LIMIT 10
    """, p):
        print(f"  {lat / 1000:9.2f} ms  {ts:%H:%M:%S}  {st}  {rt:22s}  {str(ua)[:32]}")

    # 未匹配路由
    unmatched = q(con, f"""
        SELECT raw_path, count(*) c FROM requests
        WHERE {where} AND route IS NULL GROUP BY ALL ORDER BY c DESC LIMIT 10
    """, p)
    if unmatched:
        section("未匹配路由（扫描器 / 写错路径）")
        for path, c in unmatched:
            print(f"  {c:7,d}  {str(path)[:70]}")

    return out


def main():
    ap = argparse.ArgumentParser(description="分析 AMLL TTML API 的请求统计数据")
    ap.add_argument("--date", help="分析这一天（YYYY-MM-DD，北京时间）")
    ap.add_argument("--since", help="起始日（含）")
    ap.add_argument("--until", help="结束日（含）")
    ap.add_argument("--json", action="store_true", help="只输出 JSON 摘要")
    args = ap.parse_args()

    con = connect()
    since, until, days = resolve_range(con, args)
    if not args.json:
        print(f"数据文件覆盖的日期：{', '.join(d.isoformat() for d in days)}")

    if args.json:
        import contextlib
        import io

        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            out = report(con, since, until)
        print(json.dumps(out, ensure_ascii=False, indent=2, default=str))
    else:
        report(con, since, until)


if __name__ == "__main__":
    main()
