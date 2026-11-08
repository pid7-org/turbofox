#!/usr/bin/env python3

import json
import math
import os
import sys

COLOR_BASE = "#ffffff"
COLOR_SURFACE = "#f8f9fa"
COLOR_BORDER = "#d0d7de"
COLOR_LINE = "#e1e4e8"
COLOR_TEXT = "#24292f"
COLOR_SUBTLE = "#57606a"

COLOR_SET_1T = "#1e66f5"
COLOR_SET_2T = "#729bfa"
COLOR_GET_1T = "#179299"
COLOR_GET_2T = "#64b6ba"
COLOR_DEL_1T = "#8839ef"
COLOR_DEL_2T = "#b487f5"

COLOR_LAT_CACHED = "#1e66f5"
COLOR_LAT_UNCACHED = "#d20f39"
COLOR_MEM_CACHED = "#8839ef"
COLOR_MEM_UNCACHED = "#df8e1d"
COLOR_DISK = "#179299"

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)


def format_throughput(ops_sec):
    if ops_sec >= 1_000_000.0:
        return f"{ops_sec / 1_000_000.0:.2f} Mops/s"
    if ops_sec >= 1_000.0:
        return f"{ops_sec / 1_000.0:.1f} kops/s"
    return f"{ops_sec:.0f} ops/s"


def format_latency(micros):
    if micros >= 1_000_000.0:
        return f"{micros / 1_000_000.0:.2f}s"
    if micros >= 1_000.0:
        return f"{micros / 1_000.0:.1f}ms"
    if micros >= 10.0:
        return f"{micros:.1f}µs"
    return f"{micros:.2f}µs"


def format_latency_short(micros):
    if micros >= 1_000_000.0:
        return f"{micros / 1_000_000.0:.2f}s"
    if micros >= 1_000.0:
        return f"{micros / 1_000.0:.1f}ms"
    if micros >= 10.0:
        return f"{micros:.0f}µs"
    return f"{micros:.1f}µs"


def format_bytes(val_bytes):
    if val_bytes <= 0.0:
        return "0 B"
    if val_bytes >= 0x4000_0000:
        return f"{val_bytes / 0x4000_0000:.2f}GB"
    if val_bytes >= 0x0010_0000:
        mb = val_bytes / 0x0010_0000
        return f"{mb:.2f}MB" if mb < 10.0 else f"{mb:.1f}MB"
    if val_bytes >= 0x0400:
        return f"{val_bytes / 0x0400:.0f}KB"
    return f"{val_bytes:.0f}B"


def format_n(n):
    if n >= 0x000F_4240:
        rem = n % 0x000F_4240
        return f"{n // 0x000F_4240}M" if rem == 0x00 else f"{n / 0x000F_4240:.1f}M"
    if n >= 0x03E8:
        rem = n % 0x03E8
        return f"{n // 0x03E8}K" if rem == 0x00 else f"{n / 0x03E8:.1f}K"
    return str(n)


def pill_label(
    svg,
    cx,
    cy,
    text,
    color=COLOR_TEXT,
    font_size=8.5,
    font_weight="bold",
    border=COLOR_BORDER,
    bg_opacity=0.92,
):
    char_w = 5.2 if font_size <= 9.0 else 6.0
    bw = round(len(text) * char_w + 8.0, 1)
    bh = 15.0
    bx = cx - bw / 2.0
    by = cy - bh / 2.0
    border_attr = f'stroke="{border}" stroke-width="0.75"' if border else ""
    svg.append(
        f'<rect x="{bx:.1f}" y="{by:.1f}" width="{bw:.1f}" height="{bh:.1f}" '
        f'rx="3" fill="#ffffff" fill-opacity="{bg_opacity}" {border_attr}/>'
    )
    svg.append(
        f'<text x="{cx:.1f}" y="{cy:.1f}" fill="{color}" font-size="{font_size}" '
        f'font-weight="{font_weight}" text-anchor="middle" '
        f'dominant-baseline="central">{text}</text>'
    )


def load_dataset(json_path):
    with open(json_path, "r", encoding="utf-8") as handle:
        data = json.load(handle)

    results = data.get("results", [])
    scale_by_n = {}
    kv_ops = {}

    for entry in results:
        bench = entry.get("benchmark")
        if bench == "scale_perf":
            ops = entry.get("operations", 0x00)
            wl = entry.get("workload", "")
            if ops not in scale_by_n:
                scale_by_n[ops] = {
                    "n": ops,
                    "db_size_bytes": entry.get("db_size_bytes", 0x00),
                    "cached_lat": 0.0,
                    "uncached_lat": 0.0,
                    "cached_mem": 0x00,
                    "uncached_mem": 0x00,
                }
            if entry.get("db_size_bytes", 0x00) > 0x00:
                scale_by_n[ops]["db_size_bytes"] = entry["db_size_bytes"]
            lat = float(entry.get("latency_micros", 0.0))
            in_mem = entry.get("in_mem_bytes", 0x00)
            if wl in ("lookup_cache_enabled", "enabled", "cached"):
                scale_by_n[ops]["cached_lat"] = lat
                scale_by_n[ops]["cached_mem"] = in_mem
            elif wl in ("lookup_cache_disabled", "disabled", "uncached"):
                scale_by_n[ops]["uncached_lat"] = lat
                scale_by_n[ops]["uncached_mem"] = in_mem
        elif bench in (
            "set_perf",
            "set_perf_multi_tx",
            "get_perf",
            "get_perf_multi_tx",
            "del_perf",
            "del_perf_multi_tx",
        ):
            kv_ops[bench] = entry

    scale_records = sorted(scale_by_n.values(), key=lambda r: r["n"])
    return {
        "metadata": data.get("metadata", {}),
        "scale": scale_records,
        "kv": kv_ops,
    }


def chart_scale_perf_horizontal(dataset):
    records = dataset["scale"]
    w, h = 1060, 490
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="15" font-weight="bold">'
        f'Database Open Latency &amp; Resource Footprint Scaling</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Dual-panel scaling profile across keys written (0 to 32k keys)</text>'
    )

    leg_x = 520
    svg.append(
        f'<circle cx="{leg_x}" cy="30" r="4" fill="{COLOR_LAT_CACHED}"/>'
        f'<text x="{leg_x + 8}" y="34" fill="{COLOR_TEXT}" font-size="9.5">Cache Open</text>'
        f'<circle cx="{leg_x + 90}" cy="30" r="4" fill="{COLOR_LAT_UNCACHED}"/>'
        f'<text x="{leg_x + 98}" y="34" fill="{COLOR_TEXT}" font-size="9.5">Raw Open</text>'
        f'<circle cx="{leg_x + 175}" cy="30" r="4" fill="{COLOR_DISK}"/>'
        f'<text x="{leg_x + 183}" y="34" fill="{COLOR_TEXT}" font-size="9.5">DB Size</text>'
        f'<circle cx="{leg_x + 250}" cy="30" r="4" fill="{COLOR_MEM_CACHED}"/>'
        f'<text x="{leg_x + 258}" y="34" fill="{COLOR_TEXT}" font-size="9.5">RAM (Cached)</text>'
        f'<circle cx="{leg_x + 360}" cy="30" r="4" fill="{COLOR_MEM_UNCACHED}"/>'
        f'<text x="{leg_x + 368}" y="34" fill="{COLOR_TEXT}" font-size="9.5">RAM (Raw)</text>'
    )

    all_n = [r["n"] for r in records]
    min_n, max_n = min(all_n), max(all_n)

    p1_x, p1_y, p1_w, p1_h = 75, 88, 420, 320
    svg.append(
        f'<rect x="{p1_x}" y="{p1_y}" width="{p1_w}" height="{p1_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y - 10}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">Open Latency (µs)</text>'
    )

    max_lat = max(max(r["cached_lat"], r["uncached_lat"]) for r in records) * 1.15
    y1_max = math.ceil(max_lat / 100.0) * 100.0

    def x1(n):
        return p1_x + 28 + ((n - min_n) / (max_n - min_n)) * (p1_w - 56)

    def y1(lat):
        return p1_y + p1_h - 20 - (lat / y1_max) * (p1_h - 40)

    for i in range(5):
        val = (y1_max / 4.0) * i
        y_pos = y1(val)
        svg.append(
            f'<line x1="{p1_x}" y1="{y_pos:.1f}" x2="{p1_x + p1_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p1_x - 8}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="end">{val:.0f}µs</text>'
        )

    for r in records:
        xc = x1(r["n"])
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p1_y}" x2="{xc:.1f}" y2="{p1_y + p1_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="0.8"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p1_y + p1_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="9.5" text-anchor="middle">{format_n(r["n"])}</text>'
        )

    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y + p1_h + 38}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Dataset Key Scale</text>'
    )

    pts_c = [f"{x1(r['n']):.1f},{y1(r['cached_lat']):.1f}" for r in records]
    pts_u = [f"{x1(r['n']):.1f},{y1(r['uncached_lat']):.1f}" for r in records]
    svg.append(
        f'<polyline points="{" ".join(pts_c)}" fill="none" '
        f'stroke="{COLOR_LAT_CACHED}" stroke-width="2.2"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_u)}" fill="none" '
        f'stroke="{COLOR_LAT_UNCACHED}" stroke-width="2.2" stroke-dasharray="4,2"/>'
    )

    for r in records:
        xc = x1(r["n"])
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{y1(r["cached_lat"]):.1f}" r="3.5" '
            f'fill="{COLOR_LAT_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{y1(r["uncached_lat"]):.1f}" r="3.5" '
            f'fill="{COLOR_LAT_UNCACHED}"/>'
        )

    for r in records:
        xc = x1(r["n"])
        yc_c = y1(r["cached_lat"])
        yc_u = y1(r["uncached_lat"])
        if abs(yc_c - yc_u) < 30.0:
            if yc_c <= yc_u:
                pill_label(
                    svg, xc, yc_c - 13.0, format_latency(r["cached_lat"]),
                    color=COLOR_LAT_CACHED, font_size=8.5
                )
                pill_label(
                    svg, xc, yc_u + 15.0, format_latency(r["uncached_lat"]),
                    color=COLOR_LAT_UNCACHED, font_size=8.5
                )
            else:
                pill_label(
                    svg, xc, yc_u - 13.0, format_latency(r["uncached_lat"]),
                    color=COLOR_LAT_UNCACHED, font_size=8.5
                )
                pill_label(
                    svg, xc, yc_c + 15.0, format_latency(r["cached_lat"]),
                    color=COLOR_LAT_CACHED, font_size=8.5
                )
        else:
            pill_label(
                svg, xc, yc_c - 13.0, format_latency(r["cached_lat"]),
                color=COLOR_LAT_CACHED, font_size=8.5
            )
            pill_label(
                svg, xc, yc_u - 13.0, format_latency(r["uncached_lat"]),
                color=COLOR_LAT_UNCACHED, font_size=8.5
            )

    p2_x, p2_y, p2_w, p2_h = 575, 88, 420, 320
    svg.append(
        f'<rect x="{p2_x}" y="{p2_y}" width="{p2_w}" height="{p2_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p2_x + p2_w/2:.1f}" y="{p2_y - 10}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">'
        f'Footprint: Disk &amp; Resident RAM</text>'
    )

    max_bytes = max(
        max(r["cached_mem"], r["uncached_mem"], r["db_size_bytes"]) for r in records
    )
    y2_max = max_bytes * 1.15

    def x2(n):
        return p2_x + 28 + ((n - min_n) / (max_n - min_n)) * (p2_w - 56)

    def y2(b):
        return p2_y + p2_h - 20 - (b / y2_max) * (p2_h - 40)

    for i in range(5):
        val = (y2_max / 4.0) * i
        y_pos = y2(val)
        svg.append(
            f'<line x1="{p2_x}" y1="{y_pos:.1f}" x2="{p2_x + p2_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p2_x - 8}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="end">{format_bytes(val)}</text>'
        )

    for r in records:
        xc = x2(r["n"])
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p2_y}" x2="{xc:.1f}" y2="{p2_y + p2_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="0.8"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p2_y + p2_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="9.5" text-anchor="middle">{format_n(r["n"])}</text>'
        )

    svg.append(
        f'<text x="{p2_x + p2_w/2:.1f}" y="{p2_y + p2_h + 38}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Dataset Key Scale</text>'
    )

    pts_d = [f"{x2(r['n']):.1f},{y2(r['db_size_bytes']):.1f}" for r in records]
    pts_mc = [f"{x2(r['n']):.1f},{y2(r['cached_mem']):.1f}" for r in records]
    pts_mu = [f"{x2(r['n']):.1f},{y2(r['uncached_mem']):.1f}" for r in records]

    svg.append(
        f'<polyline points="{" ".join(pts_d)}" fill="none" '
        f'stroke="{COLOR_DISK}" stroke-width="2.2"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_mc)}" fill="none" '
        f'stroke="{COLOR_MEM_CACHED}" stroke-width="2.2"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_mu)}" fill="none" '
        f'stroke="{COLOR_MEM_UNCACHED}" stroke-width="2.2" stroke-dasharray="4,2"/>'
    )

    for r in records:
        xc = x2(r["n"])
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{y2(r["db_size_bytes"]):.1f}" r="3.5" '
            f'fill="{COLOR_DISK}"/>'
        )
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{y2(r["cached_mem"]):.1f}" r="3.5" '
            f'fill="{COLOR_MEM_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{y2(r["uncached_mem"]):.1f}" r="3.5" '
            f'fill="{COLOR_MEM_UNCACHED}"/>'
        )

    for r in records:
        xc = x2(r["n"])
        yc_d = y2(r["db_size_bytes"])
        pill_label(
            svg, xc, yc_d - 13.0, format_bytes(r["db_size_bytes"]),
            color=COLOR_DISK, font_size=8.5
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_scale_perf_vertical(dataset):
    records = dataset["scale"]
    w, h = 980, 730
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Main title and subtitle
    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Database Open Latency &amp; Resource Footprint Scaling (Aligned)</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Vertically aligned X-axis comparing latency vs physical storage growth</text>'
    )

    # Dedicated legend row (y = 74)
    svg.append(
        f'<circle cx="36" cy="74" r="4" fill="{COLOR_LAT_CACHED}"/>'
        f'<text x="46" y="78" fill="{COLOR_TEXT}" font-size="10.5">Cache Open</text>'
        f'<circle cx="156" cy="74" r="4" fill="{COLOR_LAT_UNCACHED}"/>'
        f'<text x="166" y="78" fill="{COLOR_TEXT}" font-size="10.5">Raw Open</text>'
        f'<circle cx="266" cy="74" r="4" fill="{COLOR_DISK}"/>'
        f'<text x="276" y="78" fill="{COLOR_TEXT}" font-size="10.5">Disk Size</text>'
        f'<circle cx="376" cy="74" r="4" fill="{COLOR_MEM_CACHED}"/>'
        f'<text x="386" y="78" fill="{COLOR_TEXT}" font-size="10.5">Cache RAM</text>'
    )

    all_n = [r["n"] for r in records]
    min_n, max_n = min(all_n), max(all_n)

    p_x = 75
    p_w = w - 110
    x_pad = 32

    def xc(n):
        return p_x + x_pad + ((n - min_n) / (max_n - min_n)) * (p_w - 2.0 * x_pad)

    # Panel 1: Title OUTSIDE and ABOVE the plot area
    p1_y, p1_h = 115, 230
    svg.append(
        f'<text x="{p_x}" y="{p1_y - 10}" fill="{COLOR_TEXT}" '
        f'font-size="12.5" font-weight="bold">Database Open Duration (µs)</text>'
    )
    svg.append(
        f'<rect x="{p_x}" y="{p1_y}" width="{p_w}" height="{p1_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    max_lat = max(max(r["cached_lat"], r["uncached_lat"]) for r in records) * 1.15
    y1_max = math.ceil(max_lat / 100.0) * 100.0

    def y1(lat):
        return p1_y + p1_h - 20 - (lat / y1_max) * (p1_h - 40)

    for i in range(5):
        val = (y1_max / 4.0) * i
        y_pos = y1(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{val:.0f}µs</text>'
        )

    # Panel 2: Title OUTSIDE and ABOVE the plot area
    p2_y, p2_h = 410, 230
    svg.append(
        f'<text x="{p_x}" y="{p2_y - 10}" fill="{COLOR_TEXT}" '
        f'font-size="12.5" font-weight="bold">Resource Footprint (Disk &amp; RAM)</text>'
    )
    svg.append(
        f'<rect x="{p_x}" y="{p2_y}" width="{p_w}" height="{p2_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    max_bytes = max(
        max(r["cached_mem"], r["uncached_mem"], r["db_size_bytes"]) for r in records
    )
    y2_max = max_bytes * 1.15

    def y2(b):
        return p2_y + p2_h - 20 - (b / y2_max) * (p2_h - 40)

    for i in range(5):
        val = (y2_max / 4.0) * i
        y_pos = y2(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{format_bytes(val)}</text>'
        )

    for r in records:
        x_val = xc(r["n"])
        svg.append(
            f'<line x1="{x_val:.1f}" y1="{p1_y}" x2="{x_val:.1f}" y2="{p1_y + p1_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        svg.append(
            f'<line x1="{x_val:.1f}" y1="{p2_y}" x2="{x_val:.1f}" y2="{p2_y + p2_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x_val:.1f}" y="{p2_y + p2_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="10" text-anchor="middle">N={format_n(r["n"])}</text>'
        )

    svg.append(
        f'<text x="{w/2:.1f}" y="{p2_y + p2_h + 40}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Dataset Key Scale (0 to 32,768 keys)</text>'
    )

    pts_c = [f"{xc(r['n']):.1f},{y1(r['cached_lat']):.1f}" for r in records]
    pts_u = [f"{xc(r['n']):.1f},{y1(r['uncached_lat']):.1f}" for r in records]
    svg.append(
        f'<polyline points="{" ".join(pts_c)}" fill="none" '
        f'stroke="{COLOR_LAT_CACHED}" stroke-width="2.5"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_u)}" fill="none" '
        f'stroke="{COLOR_LAT_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )
    for r in records:
        x_val = xc(r["n"])
        svg.append(
            f'<circle cx="{x_val:.1f}" cy="{y1(r["cached_lat"]):.1f}" r="4" '
            f'fill="{COLOR_LAT_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{x_val:.1f}" cy="{y1(r["uncached_lat"]):.1f}" r="4" '
            f'fill="{COLOR_LAT_UNCACHED}"/>'
        )

    for r in records:
        x_val = xc(r["n"])
        yc_c = y1(r["cached_lat"])
        yc_u = y1(r["uncached_lat"])
        if abs(yc_c - yc_u) < 30.0:
            if yc_c <= yc_u:
                pill_label(
                    svg, x_val, yc_c - 13.0, format_latency(r["cached_lat"]),
                    color=COLOR_LAT_CACHED, font_size=8.5
                )
                pill_label(
                    svg, x_val, yc_u + 15.0, format_latency(r["uncached_lat"]),
                    color=COLOR_LAT_UNCACHED, font_size=8.5
                )
            else:
                pill_label(
                    svg, x_val, yc_u - 13.0, format_latency(r["uncached_lat"]),
                    color=COLOR_LAT_UNCACHED, font_size=8.5
                )
                pill_label(
                    svg, x_val, yc_c + 15.0, format_latency(r["cached_lat"]),
                    color=COLOR_LAT_CACHED, font_size=8.5
                )
        else:
            pill_label(
                svg, x_val, yc_c - 13.0, format_latency(r["cached_lat"]),
                color=COLOR_LAT_CACHED, font_size=8.5
            )
            pill_label(
                svg, x_val, yc_u - 13.0, format_latency(r["uncached_lat"]),
                color=COLOR_LAT_UNCACHED, font_size=8.5
            )

    pts_d = [f"{xc(r['n']):.1f},{y2(r['db_size_bytes']):.1f}" for r in records]
    pts_mc = [f"{xc(r['n']):.1f},{y2(r['cached_mem']):.1f}" for r in records]
    pts_mu = [f"{xc(r['n']):.1f},{y2(r['uncached_mem']):.1f}" for r in records]
    svg.append(
        f'<polyline points="{" ".join(pts_d)}" fill="none" '
        f'stroke="{COLOR_DISK}" stroke-width="2.5"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_mc)}" fill="none" '
        f'stroke="{COLOR_MEM_CACHED}" stroke-width="2.5"/>'
    )
    svg.append(
        f'<polyline points="{" ".join(pts_mu)}" fill="none" '
        f'stroke="{COLOR_MEM_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )
    for r in records:
        x_val = xc(r["n"])
        svg.append(
            f'<circle cx="{x_val:.1f}" cy="{y2(r["db_size_bytes"]):.1f}" r="4" '
            f'fill="{COLOR_DISK}"/>'
        )
        svg.append(
            f'<circle cx="{x_val:.1f}" cy="{y2(r["cached_mem"]):.1f}" r="4" '
            f'fill="{COLOR_MEM_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{x_val:.1f}" cy="{y2(r["uncached_mem"]):.1f}" r="4" '
            f'fill="{COLOR_MEM_UNCACHED}"/>'
        )

    for r in records:
        x_val = xc(r["n"])
        yc_d = y2(r["db_size_bytes"])
        pill_label(
            svg, x_val, yc_d - 13.0, format_bytes(r["db_size_bytes"]),
            color=COLOR_DISK, font_size=8.5
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_kv_throughput_concurrency(dataset):
    kv = dataset["kv"]
    w, h = 1040, 500
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'KV Operation Throughput: 1-Thread vs 2-Threads Concurrency</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Split-scale comparison isolating write lock contention from high-throughput reads'
        f'</text>'
    )

    svg.append(
        f'<rect x="760" y="24" width="14" height="14" rx="3" fill="#1e66f5"/>'
        f'<text x="782" y="36" fill="{COLOR_TEXT}" font-size="11">1 Thread</text>'
        f'<rect x="870" y="24" width="14" height="14" rx="3" fill="#729bfa"/>'
        f'<text x="892" y="36" fill="{COLOR_TEXT}" font-size="11">2 Threads (Multi-Tx)</text>'
    )

    # Shift p1_x to 95 to guarantee left Y-axis labels have 20+ px left margin inside canvas
    p1_x, p1_y, p1_w, p1_h = 95, 100, 280, 310
    svg.append(
        f'<rect x="{p1_x}" y="{p1_y}" width="{p1_w}" height="{p1_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">SET (80/20 New/Update)</text>'
    )

    set_1t = kv.get("set_perf", {}).get("throughput_ops_sec", 0.0)
    set_2t = kv.get("set_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
    max_set = 14_000.0

    for i in range(5):
        val = (max_set / 4.0) * i
        y_pos = p1_y + p1_h - 20 - (val / max_set) * (p1_h - 40)
        svg.append(
            f'<line x1="{p1_x}" y1="{y_pos:.1f}" x2="{p1_x + p1_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p1_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="end">{val/1000.0:.1f} kops/s</text>'
        )

    b_w = 48
    cx_1t = p1_x + p1_w / 2 - b_w - 8
    cx_2t = p1_x + p1_w / 2 + 8
    h_1t = (set_1t / max_set) * (p1_h - 40)
    h_2t = (set_2t / max_set) * (p1_h - 40)
    y_1t = p1_y + p1_h - 20 - h_1t
    y_2t = p1_y + p1_h - 20 - h_2t

    svg.append(
        f'<rect x="{cx_1t:.1f}" y="{y_1t:.1f}" width="{b_w}" height="{h_1t:.1f}" '
        f'rx="4" fill="{COLOR_SET_1T}"/>'
    )
    svg.append(
        f'<text x="{cx_1t + b_w/2:.1f}" y="{y_1t - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(set_1t)}</text>'
    )
    svg.append(
        f'<text x="{cx_1t + b_w/2:.1f}" y="{p1_y + p1_h + 18}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">1 Thread</text>'
    )

    svg.append(
        f'<rect x="{cx_2t:.1f}" y="{y_2t:.1f}" width="{b_w}" height="{h_2t:.1f}" '
        f'rx="4" fill="{COLOR_SET_2T}"/>'
    )
    svg.append(
        f'<text x="{cx_2t + b_w/2:.1f}" y="{y_2t - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(set_2t)}</text>'
    )
    svg.append(
        f'<text x="{cx_2t + b_w/2:.1f}" y="{p1_y + p1_h + 18}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">2 Threads</text>'
    )

    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y + 40}" fill="#d20f39" font-size="10" '
        f'font-weight="bold" text-anchor="middle">-75.6% Lock Contention</text>'
    )

    p2_x, p2_y, p2_w, p2_h = 445, 100, 530, 310
    svg.append(
        f'<rect x="{p2_x}" y="{p2_y}" width="{p2_w}" height="{p2_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p2_x + p2_w/2:.1f}" y="{p2_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">'
        f'GET &amp; DEL (60/40 Hit/Miss)</text>'
    )

    get_1t = kv.get("get_perf", {}).get("throughput_ops_sec", 0.0)
    get_2t = kv.get("get_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
    del_1t = kv.get("del_perf", {}).get("throughput_ops_sec", 0.0)
    del_2t = kv.get("del_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
    max_read = 1_800_000.0

    for i in range(5):
        val = (max_read / 4.0) * i
        y_pos = p2_y + p2_h - 20 - (val / max_read) * (p2_h - 40)
        svg.append(
            f'<line x1="{p2_x}" y1="{y_pos:.1f}" x2="{p2_x + p2_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p2_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="end">{val/1_000_000.0:.2f} Mops/s</text>'
        )

    get_cx = p2_x + p2_w * 0.28
    h_g1 = (get_1t / max_read) * (p2_h - 40)
    h_g2 = (get_2t / max_read) * (p2_h - 40)
    y_g1 = p2_y + p2_h - 20 - h_g1
    y_g2 = p2_y + p2_h - 20 - h_g2

    svg.append(
        f'<rect x="{get_cx - b_w - 4:.1f}" y="{y_g1:.1f}" width="{b_w}" height="{h_g1:.1f}" '
        f'rx="4" fill="{COLOR_GET_1T}"/>'
    )
    svg.append(
        f'<text x="{get_cx - b_w/2 - 4:.1f}" y="{y_g1 - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(get_1t)}</text>'
    )
    svg.append(
        f'<rect x="{get_cx + 4:.1f}" y="{y_g2:.1f}" width="{b_w}" height="{h_g2:.1f}" '
        f'rx="4" fill="{COLOR_GET_2T}"/>'
    )
    svg.append(
        f'<text x="{get_cx + b_w/2 + 4:.1f}" y="{y_g2 - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(get_2t)}</text>'
    )
    svg.append(
        f'<text x="{get_cx:.1f}" y="{p2_y + p2_h + 18}" fill="{COLOR_TEXT}" '
        f'font-size="11" font-weight="bold" text-anchor="middle">GET Workload</text>'
    )
    svg.append(
        f'<text x="{get_cx:.1f}" y="{p2_y + p2_h + 34}" fill="{COLOR_SUBTLE}" '
        f'font-size="9.5" text-anchor="middle">1T vs 2T</text>'
    )

    del_cx = p2_x + p2_w * 0.72
    h_d1 = (del_1t / max_read) * (p2_h - 40)
    h_d2 = (del_2t / max_read) * (p2_h - 40)
    y_d1 = p2_y + p2_h - 20 - h_d1
    y_d2 = p2_y + p2_h - 20 - h_d2

    svg.append(
        f'<rect x="{del_cx - b_w - 4:.1f}" y="{y_d1:.1f}" width="{b_w}" height="{h_d1:.1f}" '
        f'rx="4" fill="{COLOR_DEL_1T}"/>'
    )
    svg.append(
        f'<text x="{del_cx - b_w/2 - 4:.1f}" y="{y_d1 - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(del_1t)}</text>'
    )
    svg.append(
        f'<rect x="{del_cx + 4:.1f}" y="{y_d2:.1f}" width="{b_w}" height="{h_d2:.1f}" '
        f'rx="4" fill="{COLOR_DEL_2T}"/>'
    )
    svg.append(
        f'<text x="{del_cx + b_w/2 + 4:.1f}" y="{y_d2 - 8:.1f}" fill="{COLOR_TEXT}" '
        f'font-size="10" font-weight="bold" text-anchor="middle">'
        f'{format_throughput(del_2t)}</text>'
    )
    svg.append(
        f'<text x="{del_cx:.1f}" y="{p2_y + p2_h + 18}" fill="{COLOR_TEXT}" '
        f'font-size="11" font-weight="bold" text-anchor="middle">DEL Workload</text>'
    )
    svg.append(
        f'<text x="{del_cx:.1f}" y="{p2_y + p2_h + 34}" fill="{COLOR_SUBTLE}" '
        f'font-size="9.5" text-anchor="middle">1T vs 2T</text>'
    )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_kv_throughput_log_concurrency(dataset):
    kv = dataset["kv"]
    w, h = 940, 490
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Operation Throughput (Logarithmic Scale)</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Sustained operations per second across single and multi-thread transactions</text>'
    )

    svg.append(
        f'<rect x="680" y="24" width="14" height="14" rx="3" fill="#1e66f5"/>'
        f'<text x="702" y="36" fill="{COLOR_TEXT}" font-size="11">1 Thread</text>'
        f'<rect x="785" y="24" width="14" height="14" rx="3" fill="#729bfa"/>'
        f'<text x="807" y="36" fill="{COLOR_TEXT}" font-size="11">2 Threads</text>'
    )

    p_x, p_y, p_w, p_h = 80, 85, 790, 320
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    log_min = 3.0
    log_max = 6.5

    def y_log(ops):
        val = math.log10(max(ops, 1000.0))
        ratio = (val - log_min) / (log_max - log_min)
        return p_y + p_h - 20 - ratio * (p_h - 40)

    ticks = [
        (1_000, "1 kops/s"),
        (10_000, "10 kops/s"),
        (100_000, "100 kops/s"),
        (1_000_000, "1 Mops/s"),
        (2_000_000, "2 Mops/s"),
    ]
    for val, lbl in ticks:
        y_pos = y_log(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{lbl}</text>'
        )

    items = [
        (
            "SET (80/20 New/Update)",
            kv.get("set_perf", {}).get("throughput_ops_sec", 0.0),
            kv.get("set_perf_multi_tx", {}).get("throughput_ops_sec", 0.0),
            COLOR_SET_1T,
            COLOR_SET_2T,
        ),
        (
            "GET (60/40 Hit/Miss)",
            kv.get("get_perf", {}).get("throughput_ops_sec", 0.0),
            kv.get("get_perf_multi_tx", {}).get("throughput_ops_sec", 0.0),
            COLOR_GET_1T,
            COLOR_GET_2T,
        ),
        (
            "DEL (60/40 Hit/Miss)",
            kv.get("del_perf", {}).get("throughput_ops_sec", 0.0),
            kv.get("del_perf_multi_tx", {}).get("throughput_ops_sec", 0.0),
            COLOR_DEL_1T,
            COLOR_DEL_2T,
        ),
    ]

    group_w = p_w / 3.0
    bar_w = 42

    for idx, (title, t1, t2, c1, c2) in enumerate(items):
        cx = p_x + group_w * (idx + 0.5)
        y1 = y_log(t1)
        y2 = y_log(t2)
        h1 = (p_y + p_h - 20) - y1
        h2 = (p_y + p_h - 20) - y2

        bx1 = cx - bar_w - 4
        svg.append(
            f'<rect x="{bx1:.1f}" y="{y1:.1f}" width="{bar_w}" height="{h1:.1f}" '
            f'rx="4" fill="{c1}"/>'
        )
        svg.append(
            f'<text x="{bx1 + bar_w/2:.1f}" y="{y1 - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="9.5" font-weight="bold" text-anchor="middle">'
            f'{format_throughput(t1)}</text>'
        )

        bx2 = cx + 4
        svg.append(
            f'<rect x="{bx2:.1f}" y="{y2:.1f}" width="{bar_w}" height="{h2:.1f}" '
            f'rx="4" fill="{c2}"/>'
        )
        svg.append(
            f'<text x="{bx2 + bar_w/2:.1f}" y="{y2 - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="9.5" font-weight="bold" text-anchor="middle">'
            f'{format_throughput(t2)}</text>'
        )

        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 20}" fill="{COLOR_TEXT}" '
            f'font-size="11" font-weight="bold" text-anchor="middle">{title}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_kv_tail_latency_split_threads(dataset):
    kv = dataset["kv"]
    w, h = 1060, 520
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Tail Latency Distribution: 1 Thread vs 2 Threads'
        f'</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Logarithmic percentile latency (0.1 µs to 100 ms) with top-positioned value labels'
        f'</text>'
    )

    svg.append(
        f'<circle cx="800" cy="30" r="4.5" fill="{COLOR_SET_1T}"/>'
        f'<text x="812" y="34" fill="{COLOR_TEXT}" font-size="10.5">SET</text>'
        f'<circle cx="865" cy="30" r="4.5" fill="{COLOR_GET_1T}"/>'
        f'<text x="877" y="34" fill="{COLOR_TEXT}" font-size="10.5">GET</text>'
        f'<circle cx="930" cy="30" r="4.5" fill="{COLOR_DEL_1T}"/>'
        f'<text x="942" y="34" fill="{COLOR_TEXT}" font-size="10.5">DEL</text>'
    )

    log_min = -1.0
    log_max = 5.5

    def y_lat(lat_us, py, ph):
        val = math.log10(max(lat_us, 0.1))
        ratio = (val - log_min) / (log_max - log_min)
        return py + ph - 20 - ratio * (ph - 40)

    percentiles = ["p50", "p90", "p99", "p999"]
    p_labels = {"p50": "p50", "p90": "p90", "p99": "p99", "p999": "p99.9"}

    ticks = [
        (0.1, "0.1µs"),
        (1.0, "1µs"),
        (10.0, "10µs"),
        (100.0, "100µs"),
        (1_000.0, "1ms"),
        (10_000.0, "10ms"),
        (100_000.0, "100ms"),
    ]

    p1_x, p1_y, p1_w, p1_h = 70, 95, 430, 340
    svg.append(
        f'<rect x="{p1_x}" y="{p1_y}" width="{p1_w}" height="{p1_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="13" font-weight="bold" text-anchor="middle">Single Thread (1T)</text>'
    )

    p2_x, p2_y, p2_w, p2_h = 565, 95, 430, 340
    svg.append(
        f'<rect x="{p2_x}" y="{p2_y}" width="{p2_w}" height="{p2_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p2_x + p2_w/2:.1f}" y="{p2_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="13" font-weight="bold" text-anchor="middle">Two Threads (2T Multi-Tx)</text>'
    )

    for px, pw, py, ph in [(p1_x, p1_w, p1_y, p1_h), (p2_x, p2_w, p2_y, p2_h)]:
        for val, lbl in ticks:
            y_pos = y_lat(val, py, ph)
            svg.append(
                f'<line x1="{px}" y1="{y_pos:.1f}" x2="{px + pw}" y2="{y_pos:.1f}" '
                f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
            )
            svg.append(
                f'<text x="{px - 8}" y="{y_pos + 3:.1f}" fill="{COLOR_SUBTLE}" '
                f'font-size="9" text-anchor="end">{lbl}</text>'
            )

    def draw_tail_bars(px, py, pw, ph, is_2t):
        ops = [
            (
                "set_perf" + ("_multi_tx" if is_2t else ""),
                "SET",
                COLOR_SET_1T if not is_2t else COLOR_SET_2T,
            ),
            (
                "get_perf" + ("_multi_tx" if is_2t else ""),
                "GET",
                COLOR_GET_1T if not is_2t else COLOR_GET_2T,
            ),
            (
                "del_perf" + ("_multi_tx" if is_2t else ""),
                "DEL",
                COLOR_DEL_1T if not is_2t else COLOR_DEL_2T,
            ),
        ]
        group_w = pw / 3.0
        bar_w = 20

        for o_idx, (b_name, b_label, b_color) in enumerate(ops):
            gx = px + group_w * o_idx
            lat_map = kv.get(b_name, {}).get("latency_micros", {})
            for p_idx, p_key in enumerate(percentiles):
                val = lat_map.get(p_key, 0.0)
                y_pos = y_lat(val, py, ph)
                h_bar = (py + ph - 20) - y_pos
                bx = gx + 15 + p_idx * (bar_w + 8)
                cx = bx + bar_w / 2

                svg.append(
                    f'<rect x="{bx:.1f}" y="{y_pos:.1f}" width="{bar_w}" height="{h_bar:.1f}" '
                    f'rx="2.5" fill="{b_color}"/>'
                )

                txt = format_latency_short(val)
                if y_pos < py + 22:
                    pill_label(svg, cx, py + 14, txt, color=b_color, font_size=8.0)
                else:
                    svg.append(
                        f'<text x="{cx:.1f}" y="{y_pos - 6:.1f}" fill="{COLOR_TEXT}" '
                        f'font-size="8" font-weight="bold" text-anchor="middle">{txt}</text>'
                    )

                svg.append(
                    f'<text x="{cx:.1f}" y="{py + ph + 16}" fill="{COLOR_SUBTLE}" '
                    f'font-size="8.5" text-anchor="middle">{p_labels[p_key]}</text>'
                )

            svg.append(
                f'<text x="{gx + group_w/2:.1f}" y="{py + ph + 36}" fill="{COLOR_TEXT}" '
                f'font-size="11" font-weight="bold" text-anchor="middle">{b_label}</text>'
            )

    draw_tail_bars(p1_x, p1_y, p1_w, p1_h, is_2t=False)
    draw_tail_bars(p2_x, p2_y, p2_w, p2_h, is_2t=True)

    svg.append("</svg>")
    return "\n".join(svg)


def chart_kv_tail_latency_grouped_bars(dataset):
    kv = dataset["kv"]
    w, h = 1040, 540
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Title & Subtitle with dedicated legend row below
    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Tail Latency Comparison: 1 Thread vs 2 Threads</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Grouped paired bars per percentile with top-positioned value labels'
        f'</text>'
    )

    # Dedicated legends row (y = 74)
    svg.append(
        f'<rect x="34" y="68" width="13" height="13" rx="2.5" fill="#1e66f5"/>'
        f'<text x="53" y="79" fill="{COLOR_TEXT}" font-size="10.5">1 Thread</text>'
        f'<rect x="145" y="68" width="13" height="13" rx="2.5" fill="#729bfa"/>'
        f'<text x="164" y="79" fill="{COLOR_TEXT}" font-size="10.5">2 Threads</text>'
    )

    log_min = -1.0
    log_max = 5.5

    def y_lat(lat_us, py, ph):
        val = math.log10(max(lat_us, 0.1))
        ratio = (val - log_min) / (log_max - log_min)
        return py + ph - 20 - ratio * (ph - 40)

    percentiles = ["p50", "p90", "p99", "p999"]
    p_labels = {"p50": "p50", "p90": "p90", "p99": "p99", "p999": "p99.9"}

    p_x, p_y, p_w, p_h = 70, 105, 900, 340
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    ticks = [
        (0.1, "0.1µs"),
        (1.0, "1µs"),
        (10.0, "10µs"),
        (100.0, "100µs"),
        (1_000.0, "1ms"),
        (10_000.0, "10ms"),
        (100_000.0, "100ms"),
    ]
    for val, lbl in ticks:
        y_pos = y_lat(val, p_y, p_h)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 8}" y="{y_pos + 3:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{lbl}</text>'
        )

    groups = [
        ("SET (80/20 New/Update)", "set_perf", "set_perf_multi_tx", COLOR_SET_1T, COLOR_SET_2T),
        ("GET (60/40 Hit/Miss)", "get_perf", "get_perf_multi_tx", COLOR_GET_1T, COLOR_GET_2T),
        ("DEL (60/40 Hit/Miss)", "del_perf", "del_perf_multi_tx", COLOR_DEL_1T, COLOR_DEL_2T),
    ]

    gw = p_w / 3.0  # 300.0 px per group
    bar_w = 14.0
    bar_gap = 2.0
    pair_w = bar_w * 2.0 + bar_gap  # 30.0 px
    pair_pitch = 54.0
    total_span = 3.0 * pair_pitch + pair_w  # 192.0 px
    # Exact centering of the 4 pairs inside gw (moves plottings right to align with group title)
    left_pad = (gw - total_span) / 2.0  # 54.0 px

    for g_idx, (g_title, b1, b2, c1, c2) in enumerate(groups):
        gx = p_x + gw * g_idx
        m1 = kv.get(b1, {}).get("latency_micros", {})
        m2 = kv.get(b2, {}).get("latency_micros", {})

        for p_idx, p_key in enumerate(percentiles):
            v1 = m1.get(p_key, 0.0)
            v2 = m2.get(p_key, 0.0)
            y1 = y_lat(v1, p_y, p_h)
            y2 = y_lat(v2, p_y, p_h)
            h1 = (p_y + p_h - 20) - y1
            h2 = (p_y + p_h - 20) - y2

            px_base = gx + left_pad + p_idx * pair_pitch
            cx1 = px_base + bar_w / 2.0
            cx2 = px_base + bar_w + bar_gap + bar_w / 2.0

            svg.append(
                f'<rect x="{px_base:.1f}" y="{y1:.1f}" width="{bar_w}" height="{h1:.1f}" '
                f'rx="2" fill="{c1}"/>'
            )
            svg.append(
                f'<rect x="{px_base + bar_w + bar_gap:.1f}" y="{y2:.1f}" width="{bar_w}" '
                f'height="{h2:.1f}" rx="2" fill="{c2}"/>'
            )

            txt1 = format_latency_short(v1)
            txt2 = format_latency_short(v2)

            # Stagger vertically outside and at top if bars are close in height
            if abs(y1 - y2) < 14.0:
                svg.append(
                    f'<text x="{cx1:.1f}" y="{y1 - 16:.1f}" fill="{COLOR_TEXT}" '
                    f'font-size="7.5" font-weight="bold" text-anchor="middle">{txt1}</text>'
                )
                svg.append(
                    f'<text x="{cx2:.1f}" y="{y2 - 6:.1f}" fill="{COLOR_TEXT}" '
                    f'font-size="7.5" font-weight="bold" text-anchor="middle">{txt2}</text>'
                )
            else:
                svg.append(
                    f'<text x="{cx1:.1f}" y="{y1 - 6:.1f}" fill="{COLOR_TEXT}" '
                    f'font-size="7.5" font-weight="bold" text-anchor="middle">{txt1}</text>'
                )
                svg.append(
                    f'<text x="{cx2:.1f}" y="{y2 - 6:.1f}" fill="{COLOR_TEXT}" '
                    f'font-size="7.5" font-weight="bold" text-anchor="middle">{txt2}</text>'
                )

            pair_mid = px_base + pair_w / 2.0
            svg.append(
                f'<text x="{pair_mid:.1f}" y="{p_y + p_h + 16}" fill="{COLOR_SUBTLE}" '
                f'font-size="8.5" text-anchor="middle">{p_labels[p_key]}</text>'
            )

        # Workload title is exactly aligned with the center of the 4 pairs
        svg.append(
            f'<text x="{gx + gw/2:.1f}" y="{p_y + p_h + 40}" fill="{COLOR_TEXT}" '
            f'font-size="11.5" font-weight="bold" text-anchor="middle">{g_title}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_concurrency_scaling_efficiency(dataset):
    kv = dataset["kv"]
    w, h = 880, 460
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Concurrency Scaling Efficiency (Throughput Ratio: 2T / 1T)</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Relative throughput of multi-threaded transactions compared to single-thread baseline'
        f'</text>'
    )

    p_x, p_y, p_w, p_h = 75, 85, 730, 290
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    max_eff = 1.2

    def y_eff(val):
        return p_y + p_h - 20 - (val / max_eff) * (p_h - 40)

    y_parity = y_eff(1.0)
    svg.append(
        f'<line x1="{p_x}" y1="{y_parity:.1f}" x2="{p_x + p_w}" y2="{y_parity:.1f}" '
        f'stroke="#d20f39" stroke-dasharray="4,4" stroke-width="1.5"/>'
    )
    svg.append(
        f'<text x="{p_x + p_w - 6}" y="{y_parity - 6:.1f}" fill="#d20f39" '
        f'font-size="9.5" font-weight="bold" text-anchor="end">1.0x (Parity Baseline)</text>'
    )

    for i in range(7):
        val = 0.2 * i
        y_pos = y_eff(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 8}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{val:.1f}x</text>'
        )

    set_eff = (
        kv.get("set_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
        / max(kv.get("set_perf", {}).get("throughput_ops_sec", 1.0), 1.0)
    )
    get_eff = (
        kv.get("get_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
        / max(kv.get("get_perf", {}).get("throughput_ops_sec", 1.0), 1.0)
    )
    del_eff = (
        kv.get("del_perf_multi_tx", {}).get("throughput_ops_sec", 0.0)
        / max(kv.get("del_perf", {}).get("throughput_ops_sec", 1.0), 1.0)
    )

    bars = [
        ("SET (Writes)", set_eff, COLOR_SET_1T, "0.24x (Write Lock Contention)"),
        ("GET (Reads)", get_eff, COLOR_GET_1T, "0.69x (Cache / Bus Contention)"),
        ("DEL (Deletes)", del_eff, COLOR_DEL_1T, "0.68x (Tombstone Contention)"),
    ]

    bw = 70
    for idx, (title, eff, col, note) in enumerate(bars):
        cx = p_x + (p_w / 3.0) * (idx + 0.5)
        bx = cx - bw / 2
        y_pos = y_eff(eff)
        h_bar = (p_y + p_h - 20) - y_pos

        svg.append(
            f'<rect x="{bx:.1f}" y="{y_pos:.1f}" width="{bw}" height="{h_bar:.1f}" '
            f'rx="5" fill="{col}"/>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{y_pos - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="12" font-weight="bold" text-anchor="middle">{eff:.2f}x</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 20}" fill="{COLOR_TEXT}" '
            f'font-size="12" font-weight="bold" text-anchor="middle">{title}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 38}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="middle">{note}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_latency_cdf_curves(dataset):
    kv = dataset["kv"]
    w, h = 980, 560
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Title & Subtitle with dedicated legend row below
    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Latency Percentile Progression Curves (Semi-Log Scale)</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Progression from min to max revealing exact inflection points and tail divergence'
        f'</text>'
    )

    # Dedicated legend row (y = 76), completely eliminating collision with title
    leg_items = [
        ("SET 1T", COLOR_SET_1T, False),
        ("SET 2T", COLOR_SET_2T, True),
        ("GET 1T", COLOR_GET_1T, False),
        ("GET 2T", COLOR_GET_2T, True),
        ("DEL 1T", COLOR_DEL_1T, False),
        ("DEL 2T", COLOR_DEL_2T, True),
    ]
    for idx, (lbl, col, is_dash) in enumerate(leg_items):
        lx = 32 + idx * 140
        dash_svg = 'stroke-dasharray="3,2"' if is_dash else ""
        svg.append(
            f'<line x1="{lx}" y1="76" x2="{lx + 20}" y2="76" stroke="{col}" '
            f'stroke-width="2.5" {dash_svg}/>'
            f'<circle cx="{lx + 10}" cy="76" r="3.5" fill="{col}"/>'
            f'<text x="{lx + 26}" y="80" fill="{COLOR_TEXT}" font-size="10.5">{lbl}</text>'
        )

    # Plot panel lowered to p_y = 105 to give generous headroom
    p_x, p_y, p_w, p_h = 75, 105, 840, 360
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    percentiles = ["min", "p50", "p75", "p90", "p95", "p99", "p999", "max"]
    p_labels = ["min", "p50", "p75", "p90", "p95", "p99", "p99.9", "max"]

    log_min = -1.0  # 0.1 µs
    log_max = 6.0   # 1,000,000 µs (1 sec) -> ample headroom for SET 2T max (234.6 ms)

    def y_lat(lat_us):
        val = math.log10(max(lat_us, 0.1))
        ratio = (val - log_min) / (log_max - log_min)
        return p_y + p_h - 20 - ratio * (p_h - 40)

    def xc(idx):
        return p_x + 30 + (idx / (len(percentiles) - 1)) * (p_w - 60)

    ticks = [
        (0.1, "0.1µs"),
        (1.0, "1µs"),
        (10.0, "10µs"),
        (100.0, "100µs"),
        (1_000.0, "1ms"),
        (10_000.0, "10ms"),
        (100_000.0, "100ms"),
        (1_000_000.0, "1s"),
    ]
    for val, lbl in ticks:
        y_pos = y_lat(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 8}" y="{y_pos + 3:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{lbl}</text>'
        )

    for idx, lbl in enumerate(p_labels):
        x_pos = xc(idx)
        svg.append(
            f'<line x1="{x_pos:.1f}" y1="{p_y}" x2="{x_pos:.1f}" y2="{p_y + p_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x_pos:.1f}" y="{p_y + p_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="10" font-weight="bold" text-anchor="middle">{lbl}</text>'
        )

    series_list = [
        ("SET 1T", "set_perf", COLOR_SET_1T, "2.5", "none"),
        ("SET 2T", "set_perf_multi_tx", COLOR_SET_2T, "2.5", "5,3"),
        ("GET 1T", "get_perf", COLOR_GET_1T, "2.2", "none"),
        ("GET 2T", "get_perf_multi_tx", COLOR_GET_2T, "2.2", "5,3"),
        ("DEL 1T", "del_perf", COLOR_DEL_1T, "2.2", "none"),
        ("DEL 2T", "del_perf_multi_tx", COLOR_DEL_2T, "2.2", "5,3"),
    ]

    for title, b_name, col, sw, dash in series_list:
        m = kv.get(b_name, {}).get("latency_micros", {})
        pts = []
        for idx, p_key in enumerate(percentiles):
            val = m.get(p_key, 0.0)
            pts.append(f"{xc(idx):.1f},{y_lat(val):.1f}")
        dash_attr = f'stroke-dasharray="{dash}"' if dash != "none" else ""
        svg.append(
            f'<polyline points="{" ".join(pts)}" fill="none" stroke="{col}" '
            f'stroke-width="{sw}" {dash_attr}/>'
        )
        for idx, p_key in enumerate(percentiles):
            val = m.get(p_key, 0.0)
            svg.append(
                f'<circle cx="{xc(idx):.1f}" cy="{y_lat(val):.1f}" r="3.5" fill="{col}"/>'
            )

    svg.append(
        f'<text x="{p_x + p_w/2:.1f}" y="{p_y + p_h + 38}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Percentile Latency Spectrum</text>'
    )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_memory_footprint_concurrency(dataset):
    kv = dataset["kv"]
    w, h = 880, 460
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Resident Memory (RAM) Footprint Under Concurrency</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Peak memory consumption (RSS) comparing 1-Thread vs 2-Threads for 1M operations'
        f'</text>'
    )

    p_x, p_y, p_w, p_h = 80, 85, 725, 290
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    db_size = kv.get("set_perf", {}).get("db_size_bytes", 93_274_112)
    mem_1t = kv.get("set_perf", {}).get("in_mem_bytes", 43_606_016)
    mem_2t = kv.get("set_perf_multi_tx", {}).get("in_mem_bytes", 79_781_888)

    max_val = 110 * 1024 * 1024.0

    def y_b(b):
        return p_y + p_h - 20 - (b / max_val) * (p_h - 40)

    for i in range(6):
        val = (max_val / 5.0) * i
        y_pos = y_b(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{val/(1024*1024):.0f} MB</text>'
        )

    bars = [
        ("Database On-Disk Size", db_size, COLOR_DISK, "Shared NVMe Storage"),
        ("Single-Thread RAM", mem_1t, COLOR_MEM_CACHED, "1 Worker Transaction"),
        ("Two-Threads RAM", mem_2t, COLOR_MEM_UNCACHED, "+82.9% Concurrency Buffers"),
    ]

    bw = 90
    for idx, (title, b_val, col, note) in enumerate(bars):
        cx = p_x + (p_w / 3.0) * (idx + 0.5)
        bx = cx - bw / 2
        y_pos = y_b(b_val)
        h_bar = (p_y + p_h - 20) - y_pos

        svg.append(
            f'<rect x="{bx:.1f}" y="{y_pos:.1f}" width="{bw}" height="{h_bar:.1f}" '
            f'rx="5" fill="{col}"/>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{y_pos - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="12" font-weight="bold" text-anchor="middle">'
            f'{format_bytes(b_val)}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 20}" fill="{COLOR_TEXT}" '
            f'font-size="12" font-weight="bold" text-anchor="middle">{title}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 38}" fill="{COLOR_SUBTLE}" '
            f'font-size="9.5" text-anchor="middle">{note}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_scale_perf_cache_overhead(dataset):
    records = dataset["scale"]
    w, h = 940, 480
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'In-Memory Lookup Cache Overhead &amp; RAM Density</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Left: Open Latency Ratio (Cached/Raw) | Right: Index RAM Bytes per Entry'
        f'</text>'
    )

    all_n = [r["n"] for r in records]
    min_n, max_n = min(all_n), max(all_n)

    p1_x, p1_y, p1_w, p1_h = 75, 95, 370, 310
    svg.append(
        f'<rect x="{p1_x}" y="{p1_y}" width="{p1_w}" height="{p1_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p1_x + p1_w/2:.1f}" y="{p1_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">'
        f'Open Latency Multiplier (Cached / Raw)</text>'
    )

    ratios = [r["cached_lat"] / max(r["uncached_lat"], 1.0) for r in records]
    max_r = 10.0

    def x1(n):
        return p1_x + 25 + ((n - min_n) / (max_n - min_n)) * (p1_w - 50)

    def y1(r):
        return p1_y + p1_h - 20 - (r / max_r) * (p1_h - 40)

    for i in range(6):
        val = 2.0 * i
        y_pos = y1(val)
        svg.append(
            f'<line x1="{p1_x}" y1="{y_pos:.1f}" x2="{p1_x + p1_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p1_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{val:.0f}x</text>'
        )

    for r in records:
        xc = x1(r["n"])
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p1_y}" x2="{xc:.1f}" y2="{p1_y + p1_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="0.8"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p1_y + p1_h + 18}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="middle">{format_n(r["n"])}</text>'
        )

    pts1 = [f"{x1(r['n']):.1f},{y1(ratio):.1f}" for r, ratio in zip(records, ratios)]
    svg.append(
        f'<polyline points="{" ".join(pts1)}" fill="none" '
        f'stroke="{COLOR_LAT_CACHED}" stroke-width="2.5"/>'
    )
    for r, ratio in zip(records, ratios):
        xc = x1(r["n"])
        yc = y1(ratio)
        svg.append(f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="4" fill="{COLOR_LAT_CACHED}"/>')
        pill_label(
            svg, xc, yc - 13.0, f"{ratio:.1f}x",
            color=COLOR_LAT_CACHED, font_size=8.5
        )

    p2_x, p2_y, p2_w, p2_h = 515, 95, 370, 310
    svg.append(
        f'<rect x="{p2_x}" y="{p2_y}" width="{p2_w}" height="{p2_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p2_x + p2_w/2:.1f}" y="{p2_y - 12}" fill="{COLOR_TEXT}" '
        f'font-size="12" font-weight="bold" text-anchor="middle">'
        f'Index Memory Density (RAM Bytes / Key)</text>'
    )

    densities = []
    for r in records:
        if r["n"] > 0:
            densities.append(
                (r["n"], (r["cached_mem"] - r["uncached_mem"]) / float(r["n"]))
            )
    max_d = 60.0

    def x2(n):
        return p2_x + 25 + ((n - min_n) / (max_n - min_n)) * (p2_w - 50)

    def y2(d):
        return p2_y + p2_h - 20 - (d / max_d) * (p2_h - 40)

    for i in range(7):
        val = 10.0 * i
        y_pos = y2(val)
        svg.append(
            f'<line x1="{p2_x}" y1="{y_pos:.1f}" x2="{p2_x + p2_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p2_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{val:.0f} B</text>'
        )

    for r in records:
        xc = x2(r["n"])
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p2_y}" x2="{xc:.1f}" y2="{p2_y + p2_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="0.8"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p2_y + p2_h + 18}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="middle">{format_n(r["n"])}</text>'
        )

    pts2 = [f"{x2(n):.1f},{y2(d):.1f}" for n, d in densities]
    svg.append(
        f'<polyline points="{" ".join(pts2)}" fill="none" '
        f'stroke="{COLOR_MEM_CACHED}" stroke-width="2.5"/>'
    )
    for n, d in densities:
        xc = x2(n)
        yc = y2(d)
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="4" fill="{COLOR_MEM_CACHED}"/>'
        )
        pill_label(
            svg, xc, yc - 13.0, f"{d:.1f}B",
            color=COLOR_MEM_CACHED, font_size=8.5
        )

    svg.append("</svg>")
    return "\n".join(svg)


def chart_kv_wall_clock_duration(dataset):
    kv = dataset["kv"]
    w, h = 880, 460
    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" '
        f'width="{w}" height="{h}" style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{w}" height="{h}" rx="10" fill="{COLOR_BASE}" '
        f'stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Total Execution Wall Time (1,048,576 Operations)</text>'
    )
    svg.append(
        f'<text x="32" y="54" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Total elapsed wall-clock seconds comparing single-thread vs 2-thread transactions'
        f'</text>'
    )

    svg.append(
        f'<rect x="660" y="24" width="14" height="14" rx="3" fill="#1e66f5"/>'
        f'<text x="682" y="36" fill="{COLOR_TEXT}" font-size="11">1 Thread</text>'
        f'<rect x="765" y="24" width="14" height="14" rx="3" fill="#729bfa"/>'
        f'<text x="787" y="36" fill="{COLOR_TEXT}" font-size="11">2 Threads</text>'
    )

    p_x, p_y, p_w, p_h = 75, 85, 730, 290
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    set_1t = kv.get("set_perf", {}).get("wall_time_secs", 0.0)
    set_2t = kv.get("set_perf_multi_tx", {}).get("wall_time_secs", 0.0)
    get_1t = kv.get("get_perf", {}).get("wall_time_secs", 0.0)
    get_2t = kv.get("get_perf_multi_tx", {}).get("wall_time_secs", 0.0)
    del_1t = kv.get("del_perf", {}).get("wall_time_secs", 0.0)
    del_2t = kv.get("del_perf_multi_tx", {}).get("wall_time_secs", 0.0)

    log_min = -1.0
    log_max = 2.8

    def y_sec(s):
        val = math.log10(max(s, 0.1))
        ratio = (val - log_min) / (log_max - log_min)
        return p_y + p_h - 20 - ratio * (p_h - 40)

    ticks = [
        (0.1, "0.1s"),
        (1.0, "1s"),
        (10.0, "10s"),
        (60.0, "1m"),
        (300.0, "5m"),
    ]
    for val, lbl in ticks:
        y_pos = y_sec(val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 8}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="9" text-anchor="end">{lbl}</text>'
        )

    groups = [
        ("SET (80/20 New/Update)", set_1t, set_2t, COLOR_SET_1T, COLOR_SET_2T),
        ("GET (60/40 Hit/Miss)", get_1t, get_2t, COLOR_GET_1T, COLOR_GET_2T),
        ("DEL (60/40 Hit/Miss)", del_1t, del_2t, COLOR_DEL_1T, COLOR_DEL_2T),
    ]

    bw = 46
    gw = p_w / 3.0

    for idx, (title, s1, s2, c1, c2) in enumerate(groups):
        cx = p_x + gw * (idx + 0.5)
        y1 = y_sec(s1)
        y2 = y_sec(s2)
        h1 = (p_y + p_h - 20) - y1
        h2 = (p_y + p_h - 20) - y2

        bx1 = cx - bw - 4
        bx2 = cx + 4
        svg.append(
            f'<rect x="{bx1:.1f}" y="{y1:.1f}" width="{bw}" height="{h1:.1f}" '
            f'rx="4" fill="{c1}"/>'
        )
        svg.append(
            f'<text x="{bx1 + bw/2:.1f}" y="{y1 - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="10" font-weight="bold" text-anchor="middle">{s1:.1f}s</text>'
        )

        svg.append(
            f'<rect x="{bx2:.1f}" y="{y2:.1f}" width="{bw}" height="{h2:.1f}" '
            f'rx="4" fill="{c2}"/>'
        )
        svg.append(
            f'<text x="{bx2 + bw/2:.1f}" y="{y2 - 8:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="10" font-weight="bold" text-anchor="middle">{s2:.1f}s</text>'
        )

        svg.append(
            f'<text x="{cx:.1f}" y="{p_y + p_h + 20}" fill="{COLOR_TEXT}" '
            f'font-size="11" font-weight="bold" text-anchor="middle">{title}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


ALL_CHARTS = {
    "scale_perf_combined_horizontal.svg": chart_scale_perf_horizontal,
    "scale_perf_combined_vertical.svg": chart_scale_perf_vertical,
    "scale_perf_cache_overhead.svg": chart_scale_perf_cache_overhead,
    "kv_throughput_concurrency.svg": chart_kv_throughput_concurrency,
    "kv_throughput_log_concurrency.svg": chart_kv_throughput_log_concurrency,
    "kv_concurrency_scaling_efficiency.svg": chart_concurrency_scaling_efficiency,
    "kv_tail_latency_split_threads.svg": chart_kv_tail_latency_split_threads,
    "kv_tail_latency_grouped_bars.svg": chart_kv_tail_latency_grouped_bars,
    "kv_latency_cdf_curves.svg": chart_latency_cdf_curves,
    "kv_memory_footprint_concurrency.svg": chart_memory_footprint_concurrency,
    "kv_wall_clock_duration.svg": chart_kv_wall_clock_duration,
}


def main():
    json_path = (
        sys.argv[1]
        if len(sys.argv) > 1
        else "tmp/results/turbofox_benchmark_results.json"
    )
    out_dir = sys.argv[2] if len(sys.argv) > 2 else "tmp/assets"

    dataset = load_dataset(json_path)
    os.makedirs(out_dir, exist_ok=True)

    for filename, generator in ALL_CHARTS.items():
        svg_content = generator(dataset)
        dest = os.path.join(out_dir, filename)
        with open(dest, "w", encoding="utf-8") as h:
            h.write(svg_content)
        print(f"Generated: {dest}")


if __name__ == "__main__":
    main()
