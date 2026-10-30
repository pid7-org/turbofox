#!/usr/bin/env python3
"""Generate Catppuccin Latte initialization latency scaling SVG"""

import json
import os
import sys

COLOR_BASE = "#eff1f5"
COLOR_SURFACE = "#e6e9ef"
COLOR_OVERLAY = "#ccd0da"
COLOR_MUTED = "#8c8fa1"
COLOR_SUBTLE = "#6c6f85"
COLOR_TEXT = "#4c4f69"
COLOR_BORDER = "#ccd0da"
COLOR_LINE = "#bcc0cc"

COLOR_NORMAL = "#1e66f5"    # Blue

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)


def load_init_results(path):
    if not os.path.exists(path):
        return []
    with open(path, "r", encoding="utf-8") as handle:
        raw = json.load(handle)

    if isinstance(raw, dict):
        entries = raw.get("results", [])
    elif isinstance(raw, list):
        entries = raw
    else:
        entries = []

    records = []
    for entry in entries:
        if entry.get("benchmark") != "scale_perf":
            continue
        workload = entry.get("workload", "")
        if workload == "with_mem_index":
            continue
        ops = entry.get("operations", 0)
        lat = entry.get("latency_micros", {})
        rec = {
            "n": ops,
            "p50": lat.get("p50", 0.0),
            "p90": lat.get("p90", 0.0),
            "p99": lat.get("p99", 0.0),
        }
        records.append(rec)

    records.sort(key=lambda r: r["n"])
    return records


def format_latency(micros):
    if micros >= 1000.0:
        return f"{micros / 1000.0:.1f}ms"
    return f"{micros:.1f}µs"


def generate_svg(records, out_path):
    svg_w = 0x03E8  # 1000 px
    svg_h = 0x01F4  # 500 px

    p_x = 0x50
    p_y = 0x50
    p_w = svg_w - 0x78
    p_h = svg_h - 0x78
    x_pad = 0x28  # 40 px padding from plot edges

    if not records:
        return

    all_n = sorted(list(set(r["n"] for r in records)))
    min_n = min(all_n)
    max_n = max(all_n)

    max_lat = max(r["p50"] for r in records)
    max_lat = max(max_lat * 1.15, 10.0)

    def x_coord(n):
        if max_n == min_n:
            return p_x + p_w / 2
        ratio = (n - min_n) / (max_n - min_n)
        return p_x + x_pad + ratio * (p_w - 2 * x_pad)

    def y_coord(lat):
        ratio = lat / max_lat
        return p_y + p_h - (ratio * p_h)

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
        f'TurboFox Database Open Latency Scaling</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Open latency across scaling intervals (Catppuccin Latte theme)</text>'
    )

    # Legend
    leg_x = svg_w - 0x00DC  # 220 px
    svg.append(
        f'<line x1="{leg_x}" y1="34" x2="{leg_x + 24}" y2="34" '
        f'stroke="{COLOR_NORMAL}" stroke-width="2.5"/>'
    )
    svg.append(f'<circle cx="{leg_x + 12}" cy="34" r="4.5" fill="{COLOR_NORMAL}"/>')
    svg.append(
        f'<text x="{leg_x + 30}" y="38" fill="{COLOR_TEXT}" font-size="11">'
        f'Open Latency (p50)</text>'
    )

    # Plot panel background
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6"/>'
    )

    # Horizontal grid lines (5 intervals)
    for i in range(5):
        y_val = (max_lat / 4.0) * i
        y_pos = y_coord(y_val)
        y_val_ms = y_val / 1000.0
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 12}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{y_val_ms:.1f}ms</text>'
        )

    # Vertical markers for each interval N
    for n in all_n:
        xc = x_coord(n)
        svg.append(
            f'<line x1="{xc:.1f}" y1="{p_y}" x2="{xc:.1f}" y2="{p_y + p_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        label = f"{n // 1000}K" if n >= 1000 and n < 1000000 else (
            f"{n // 1000000}M" if n >= 1000000 else str(n)
        )
        svg.append(
            f'<text x="{xc:.1f}" y="{p_y + p_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="11" text-anchor="middle">N={label}</text>'
        )

    # Render series line and data points
    pts = []
    for r in records:
        xc = x_coord(r["n"])
        yc = y_coord(r["p50"])
        pts.append(f"{xc:.1f},{yc:.1f}")

    svg.append(
        f'<polyline points="{" ".join(pts)}" fill="none" '
        f'stroke="{COLOR_NORMAL}" stroke-width="2.5"/>'
    )

    for r in records:
        xc = x_coord(r["n"])
        yc = y_coord(r["p50"])
        svg.append(
            f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="4.5" fill="{COLOR_NORMAL}"/>'
        )
        offset_y = -8
        svg.append(
            f'<text x="{xc:.1f}" y="{yc + offset_y:.1f}" fill="{COLOR_NORMAL}" '
            f'font-size="10" font-weight="bold" text-anchor="middle">'
            f'{format_latency(r["p50"])}</text>'
        )

    svg.append('</svg>')

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(svg))


if __name__ == "__main__":
    in_file = sys.argv[1] if len(sys.argv) > 1 else "tmp/results/turbofox_benchmark_results.json"
    out_file = sys.argv[2] if len(sys.argv) > 2 else "assets/init_scaling.svg"
    records = load_init_results(in_file)
    generate_svg(records, out_file)
