#!/usr/bin/env python3
"""Generate scaling performance benchmark SVGs for open latency and resource footprint"""

import json
import os
import sys

COLOR_BASE = "#fff"        # Plain white background
COLOR_SURFACE = "#f8f9fa"   # Light neutral plot area surface
COLOR_BORDER = "#d0d7de"    # Container border
COLOR_LINE = "#e1e4e8"      # Grid line
COLOR_TEXT = "#24292f"      # Primary text
COLOR_SUBTLE = "#57606a"    # Secondary text and axis ticks

COLOR_LAT_CACHED = "#1e66f5"    # Blue
COLOR_LAT_UNCACHED = "#d20f39"  # Crimson / Red
COLOR_MEM_CACHED = "#8839ef"    # Mauve
COLOR_MEM_UNCACHED = "#df8e1d"  # Amber
COLOR_DISK = "#179299"          # Teal

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)


def load_scale_results(path):
    if not os.path.exists(path):
        return []
    with open(path, "r", encoding="utf-8") as handle:
        content = handle.read().strip()

    if not content:
        return []

    try:
        raw = json.loads(content)
    except json.JSONDecodeError:
        try:
            raw = json.loads(f"[{content.rstrip().rstrip(',')}]")
        except json.JSONDecodeError:
            return []

    if isinstance(raw, dict):
        entries = raw.get("results", [])
    elif isinstance(raw, list):
        entries = raw
    else:
        entries = []

    by_n = {}
    for entry in entries:
        if entry.get("benchmark") != "scale_perf":
            continue
        workload = entry.get("workload", "")
        if workload == "with_mem_index":
            continue
        ops = entry.get("operations", 0x00)
        lat_val = entry.get("latency_micros")
        if isinstance(lat_val, dict):
            lat = lat_val.get("p50", 0.0)
        elif isinstance(lat_val, (int, float)):
            lat = float(lat_val)
        else:
            lat = float(entry.get("open_latency_micros", 0.0))
        in_mem = entry.get("in_mem_bytes", 0x00)
        db_size = entry.get("db_size_bytes", 0x00)

        if ops not in by_n:
            by_n[ops] = {
                "n": ops,
                "db_size_bytes": db_size,
                "cached_lat": 0.0,
                "uncached_lat": 0.0,
                "cached_mem": 0,
                "uncached_mem": 0,
            }

        if db_size > 0:
            by_n[ops]["db_size_bytes"] = db_size

        if workload in (
            "lookup_cache_enabled",
            "enabled",
            "open_cached",
            "cached",
            "open_warm",
            "warm",
        ):
            by_n[ops]["cached_lat"] = lat
            by_n[ops]["cached_mem"] = in_mem
        elif workload in (
            "lookup_cache_disabled",
            "disabled",
            "open_uncached",
            "uncached",
            "open_cold",
            "cold",
            "open",
        ):
            by_n[ops]["uncached_lat"] = lat
            by_n[ops]["uncached_mem"] = in_mem

    records = list(by_n.values())
    records.sort(key=lambda r: r["n"])
    return records


def format_latency(micros):
    if micros >= 1000.0:
        return f"{micros / 1000.0:.1f}ms"
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


def generate_latency_svg(records, out_path):
    svg_w = 0x03E8  # 1000 px
    svg_h = 0x01F4  # 500 px

    p_x = 0x50      # 80 px
    p_y = 0x4B      # 75 px
    p_w = svg_w - 0x78  # 880 px
    p_h = 0x0168    # 360 px
    x_pad = 0x28    # 40 px horizontal pad
    y_pad = 0x14    # 20 px vertical pad

    if not records:
        return

    all_n = sorted(list(set(r["n"] for r in records)))
    min_n = min(all_n)
    max_n = max(all_n)

    min_lat = min(min(r["cached_lat"], r["uncached_lat"]) for r in records)
    max_lat = max(max(r["cached_lat"], r["uncached_lat"]) for r in records)
    y_min = max(0.0, (int(min_lat * 0.85) // 50) * 50.0)
    y_max = max((int(max_lat * 1.15) // 50 + 1) * 50.0, y_min + 100.0)

    def x_coord(n):
        if max_n == min_n:
            return p_x + p_w / 2.0
        ratio = (n - min_n) / (max_n - min_n)
        return p_x + x_pad + ratio * (p_w - 2.0 * x_pad)

    def y_coord(lat):
        if y_max == y_min:
            return p_y + p_h / 2.0
        ratio = (lat - y_min) / (y_max - y_min)
        return p_y + p_h - y_pad - (ratio * (p_h - 2.0 * y_pad))

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="12" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Title & Subtitle
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'TurboFox Open Latency Scaling</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Database open latency with lookup_cache enabled vs disabled</text>'
    )

    # Legends (dual series)
    leg_x1 = 0x01F4  # 500 px
    svg.append(
        f'<line x1="{leg_x1}" y1="34" x2="{leg_x1 + 24}" y2="34" '
        f'stroke="{COLOR_LAT_CACHED}" stroke-width="2.5"/>'
    )
    svg.append(f'<circle cx="{leg_x1 + 12}" cy="34" r="4.5" fill="{COLOR_LAT_CACHED}"/>')
    svg.append(
        f'<text x="{leg_x1 + 30}" y="38" fill="{COLOR_TEXT}" font-size="11">'
        f'lookup_cache Enabled</text>'
    )

    leg_x2 = 0x02DC  # 732 px
    svg.append(
        f'<line x1="{leg_x2}" y1="34" x2="{leg_x2 + 24}" y2="34" '
        f'stroke="{COLOR_LAT_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )
    svg.append(f'<circle cx="{leg_x2 + 12}" cy="34" r="4.5" fill="{COLOR_LAT_UNCACHED}"/>')
    svg.append(
        f'<text x="{leg_x2 + 30}" y="38" fill="{COLOR_TEXT}" font-size="11">'
        f'lookup_cache Disabled</text>'
    )

    # Plot panel background
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    # Horizontal grid lines (5 intervals)
    for i in range(0x05):
        y_val = y_min + ((y_max - y_min) / 4.0) * i
        y_pos = y_coord(y_val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 12}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{format_latency(y_val)}</text>'
        )

    # Vertical markers for each interval N
    for n in all_n:
        xc = x_coord(n)
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p_y}" x2="{xc:.1f}" y2="{p_y + p_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p_y + p_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="11" text-anchor="middle">N={format_n(n)}</text>'
        )

    # Bottom caption
    svg.append(
        f'<text x="{p_x + p_w / 2.0:.1f}" y="{p_y + p_h + 38}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Scaling Intervals (Keys Written)</text>'
    )

    # Series 1: Cached polyline
    pts_c = []
    for r in records:
        xc = x_coord(r["n"]) - 5.0
        yc = y_coord(r["cached_lat"])
        pts_c.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts_c)}" fill="none" '
        f'stroke="{COLOR_LAT_CACHED}" stroke-width="2.5"/>'
    )

    # Series 2: Uncached polyline
    pts_u = []
    for r in records:
        xc = x_coord(r["n"]) + 5.0
        yc = y_coord(r["uncached_lat"])
        pts_u.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts_u)}" fill="none" '
        f'stroke="{COLOR_LAT_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )

    # Data points and labels
    for r in records:
        xc = x_coord(r["n"])
        xc_c = xc - 5.0
        xc_u = xc + 5.0
        yc_c = y_coord(r["cached_lat"])
        yc_u = y_coord(r["uncached_lat"])

        svg.append(
            f'<circle cx="{xc_c:.1f}" cy="{yc_c:.1f}" r="4.5" fill="{COLOR_LAT_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{xc_u:.1f}" cy="{yc_u:.1f}" r="4.5" fill="{COLOR_LAT_UNCACHED}"/>'
        )

        # Place label of visually higher point above, and lower point below
        if yc_c <= yc_u:
            svg.append(
                f'<text x="{xc_c:.1f}" y="{yc_c - 9:.1f}" fill="{COLOR_LAT_CACHED}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_latency(r["cached_lat"])}</text>'
            )
            svg.append(
                f'<text x="{xc_u:.1f}" y="{yc_u + 16:.1f}" fill="{COLOR_LAT_UNCACHED}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_latency(r["uncached_lat"])}</text>'
            )
        else:
            svg.append(
                f'<text x="{xc_u:.1f}" y="{yc_u - 9:.1f}" fill="{COLOR_LAT_UNCACHED}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_latency(r["uncached_lat"])}</text>'
            )
            svg.append(
                f'<text x="{xc_c:.1f}" y="{yc_c + 16:.1f}" fill="{COLOR_LAT_CACHED}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_latency(r["cached_lat"])}</text>'
            )

    svg.append('</svg>')

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(svg))


def generate_footprint_svg(records, out_path):
    svg_w = 0x03E8  # 1000 px
    svg_h = 0x01F4  # 500 px

    p_x = 0x50      # 80 px
    p_y = 0x4B      # 75 px
    p_w = svg_w - 0x78  # 880 px
    p_h = 0x0168    # 360 px
    x_pad = 0x28    # 40 px horizontal pad
    y_pad = 0x16    # 22 px vertical pad

    if not records:
        return

    all_n = sorted(list(set(r["n"] for r in records)))
    min_n = min(all_n)
    max_n = max(all_n)

    max_bytes = max(
        max(r["cached_mem"], r["uncached_mem"], r["db_size_bytes"]) for r in records
    )
    max_bytes = max(float(max_bytes) * 1.15, 1024.0)

    def x_coord(n):
        if max_n == min_n:
            return p_x + p_w / 2.0
        ratio = (n - min_n) / (max_n - min_n)
        return p_x + x_pad + ratio * (p_w - 2.0 * x_pad)

    def y_coord(b):
        ratio = b / max_bytes
        return p_y + p_h - y_pad - (ratio * (p_h - 2.0 * y_pad))

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="12" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Title & Subtitle
    svg.append(
        f'<text x="32" y="34" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'TurboFox Resource Footprint Scaling</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Resident memory and disk footprint across intervals (lookup_cache enabled vs disabled)</text>'
    )

    # Legends (3 series)
    leg_x1 = 0x01C2  # 450 px
    svg.append(
        f'<line x1="{leg_x1}" y1="34" x2="{leg_x1 + 20}" y2="34" '
        f'stroke="{COLOR_MEM_CACHED}" stroke-width="2.5"/>'
    )
    svg.append(f'<circle cx="{leg_x1 + 10}" cy="34" r="4.0" fill="{COLOR_MEM_CACHED}"/>')
    svg.append(
        f'<text x="{leg_x1 + 25}" y="38" fill="{COLOR_TEXT}" font-size="10">'
        f'RAM (lookup_cache enabled)</text>'
    )

    leg_x2 = leg_x1 + 0x00C8  # 650 px
    svg.append(
        f'<line x1="{leg_x2}" y1="34" x2="{leg_x2 + 20}" y2="34" '
        f'stroke="{COLOR_MEM_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )
    svg.append(f'<circle cx="{leg_x2 + 10}" cy="34" r="4.0" fill="{COLOR_MEM_UNCACHED}"/>')
    svg.append(
        f'<text x="{leg_x2 + 25}" y="38" fill="{COLOR_TEXT}" font-size="10">'
        f'RAM (lookup_cache disabled)</text>'
    )

    leg_x3 = leg_x2 + 0x00CE  # 856 px
    svg.append(
        f'<line x1="{leg_x3}" y1="34" x2="{leg_x3 + 20}" y2="34" '
        f'stroke="{COLOR_DISK}" stroke-width="2.5" stroke-dasharray="2,2"/>'
    )
    svg.append(f'<circle cx="{leg_x3 + 10}" cy="34" r="4.0" fill="{COLOR_DISK}"/>')
    svg.append(
        f'<text x="{leg_x3 + 25}" y="38" fill="{COLOR_TEXT}" font-size="10">'
        f'Disk Footprint</text>'
    )

    # Plot panel background
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    # Horizontal grid lines (5 intervals)
    for i in range(0x05):
        y_val_b = (max_bytes / 4.0) * i
        y_pos = y_coord(y_val_b)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 12}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{format_bytes(y_val_b)}</text>'
        )

    # Vertical markers for each interval N
    for n in all_n:
        xc = x_coord(n)
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p_y}" x2="{xc:.1f}" y2="{p_y + p_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p_y + p_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="11" text-anchor="middle">N={format_n(n)}</text>'
        )

    # Bottom caption
    svg.append(
        f'<text x="{p_x + p_w / 2.0:.1f}" y="{p_y + p_h + 38}" fill="{COLOR_SUBTLE}" '
        f'font-size="10" text-anchor="middle">Scaling Intervals (Keys Written)</text>'
    )

    # Series 1: Enabled Memory
    pts_mem_c = []
    for r in records:
        xc = x_coord(r["n"]) - 6.0
        yc = y_coord(r["cached_mem"])
        pts_mem_c.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts_mem_c)}" fill="none" '
        f'stroke="{COLOR_MEM_CACHED}" stroke-width="2.5"/>'
    )

    # Series 2: Disabled Memory
    pts_mem_u = []
    for r in records:
        xc = x_coord(r["n"]) + 6.0
        yc = y_coord(r["uncached_mem"])
        pts_mem_u.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts_mem_u)}" fill="none" '
        f'stroke="{COLOR_MEM_UNCACHED}" stroke-width="2.5" stroke-dasharray="4,2"/>'
    )

    # Series 3: Disk Footprint
    pts_disk = []
    for r in records:
        xc = x_coord(r["n"])
        yc = y_coord(r["db_size_bytes"])
        pts_disk.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts_disk)}" fill="none" '
        f'stroke="{COLOR_DISK}" stroke-width="2.5" stroke-dasharray="2,2"/>'
    )

    # Data points and labels
    for r in records:
        xc = x_coord(r["n"])
        xc_mem_c = xc - 6.0
        xc_mem_u = xc + 6.0
        yc_mem_c = y_coord(r["cached_mem"])
        yc_mem_u = y_coord(r["uncached_mem"])
        yc_disk = y_coord(r["db_size_bytes"])

        # Tri-series circles (staggered horizontally)
        svg.append(
            f'<circle cx="{xc_mem_c:.1f}" cy="{yc_mem_c:.1f}" r="4.0" fill="{COLOR_MEM_CACHED}"/>'
        )
        svg.append(
            f'<circle cx="{xc_mem_u:.1f}" cy="{yc_mem_u:.1f}" r="4.0" fill="{COLOR_MEM_UNCACHED}"/>'
        )
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{yc_disk:.1f}" r="4.0" fill="{COLOR_DISK}"/>'
        )

        # Labels: Disk above (center), Enabled below-left, Disabled below-right
        svg.append(
            f'<text x="{xc:.1f}" y="{yc_disk - 9:.1f}" fill="{COLOR_DISK}" '
            f'font-size="9" font-weight="bold" text-anchor="middle">'
            f'{format_bytes(r["db_size_bytes"])}</text>'
        )
        svg.append(
            f'<text x="{xc - 8:.1f}" y="{yc_mem_c + 15:.1f}" fill="{COLOR_MEM_CACHED}" '
            f'font-size="9" font-weight="bold" text-anchor="end">'
            f'{format_bytes(r["cached_mem"])}</text>'
        )
        svg.append(
            f'<text x="{xc + 8:.1f}" y="{yc_mem_u + 15:.1f}" fill="{COLOR_MEM_UNCACHED}" '
            f'font-size="9" font-weight="bold" text-anchor="start">'
            f'{format_bytes(r["uncached_mem"])}</text>'
        )

    svg.append('</svg>')

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(svg))


if __name__ == "__main__":
    in_file = sys.argv[1] if len(sys.argv) > 1 else "tmp/results/turbofox_benchmark_results.json"
    if len(sys.argv) > 2 and os.path.isdir(sys.argv[2]):
        out_dir = sys.argv[2]
        latency_out = os.path.join(out_dir, "scale_perf_latency.svg")
        footprint_out = os.path.join(out_dir, "scale_perf_footprint.svg")
    else:
        latency_out = sys.argv[2] if len(sys.argv) > 2 else "assets/scale_perf_latency.svg"
        footprint_out = sys.argv[3] if len(sys.argv) > 3 else "assets/scale_perf_footprint.svg"

    records = load_scale_results(in_file)
    generate_latency_svg(records, latency_out)
    generate_footprint_svg(records, footprint_out)
