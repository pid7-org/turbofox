#!/usr/bin/env python3
"""Generate Catppuccin Latte initialization latency scaling SVG"""

import json
import math
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

COLOR_P50 = "#1e66f5" # Blue
COLOR_P90 = "#fe640b" # Peach
COLOR_P99 = "#d20f39" # Red

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)

DEFAULT_SCALES = [1000, 10000, 100000]


def load_init_results(path):
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    records = []
    for entry in data.get("results", []):
        if entry.get("benchmark") == "init_perf":
            ops = entry.get("operations", 0)
            lat = entry.get("latency_micros", {})
            records.append({
                "n": ops,
                "p50": lat.get("p50", 0.0),
                "p90": lat.get("p90", 0.0),
                "p99": lat.get("p99", 0.0),
                "p999": lat.get("p999", 0.0),
            })
    records.sort(key=lambda r: r["n"])
    return records


def generate_svg(records, out_path):
    svg_w = 0x03E8 # 1000 px
    svg_h = 0x01F4 # 500 px

    p_x = 0x50
    p_y = 0x50
    p_w = svg_w - 0x78
    p_h = svg_h - 0x78

    if not records:
        return

    n_vals = [r["n"] for r in records]
    max_lat = max(max(r["p99"], r["p90"], r["p50"]) for r in records)
    max_lat = max(max_lat * 1.25, 10.0)

    # Log10 scaling for X-axis
    log_min_x = math.log10(min(n_vals)) if min(n_vals) > 0 else 0
    log_max_x = math.log10(max(n_vals)) if max(n_vals) > min(n_vals) else (log_min_x + 1)

    def x_coord(n):
        if log_max_x == log_min_x:
            return p_x + p_w / 2
        ratio = (math.log10(n) - log_min_x) / (log_max_x - log_min_x)
        return p_x + ratio * p_w

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
        f'TurboFox Database Open Latency Scaling (O(1) Memory-Mapping Bound)</text>'
    )
    svg.append(
        f'<text x="32" y="52" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Initialization latency across entry scale N (Catppuccin Latte theme)</text>'
    )

    # Legend
    leg_x = svg_w - 0x0118
    svg.append(f'<circle cx="{leg_x}" cy="34" r="5" fill="{COLOR_P50}"/>')
    svg.append(
        f'<text x="{leg_x + 10}" y="38" fill="{COLOR_TEXT}" font-size="11">p50</text>'
    )
    svg.append(f'<circle cx="{leg_x + 60}" cy="34" r="5" fill="{COLOR_P90}"/>')
    svg.append(
        f'<text x="{leg_x + 70}" y="38" fill="{COLOR_TEXT}" font-size="11">p90</text>'
    )
    svg.append(f'<circle cx="{leg_x + 120}" cy="34" r="5" fill="{COLOR_P99}"/>')
    svg.append(
        f'<text x="{leg_x + 130}" y="38" fill="{COLOR_TEXT}" font-size="11">p99</text>'
    )

    # Plot panel background and grid
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6"/>'
    )

    # Horizontal grid lines (5 intervals)
    for i in range(5):
        y_val = (max_lat / 4.0) * i
        y_pos = y_coord(y_val)
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos}" x2="{p_x + p_w}" y2="{y_pos}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{y_val:.1f}µs</text>'
        )

    # Vertical markers for each N
    for r in records:
        xc = x_coord(r["n"])
        svg.append(
            f'<line x1="{xc}" y1="{p_y}" x2="{xc}" y2="{p_y + p_h}" '
            f'stroke="{COLOR_LINE}" stroke-width="1"/>'
        )
        label = f'{r["n"]//1000}K' if r["n"] >= 1000 and r["n"] < 1000000 else (
            f'{r["n"]//1000000}M' if r["n"] >= 1000000 else str(r["n"])
        )
        svg.append(
            f'<text x="{xc}" y="{p_y + p_h + 18}" fill="{COLOR_TEXT}" '
            f'font-size="11" text-anchor="middle">N={label}</text>'
        )

    # Plot lines & points
    for metric, color in [("p99", COLOR_P99), ("p90", COLOR_P90), ("p50", COLOR_P50)]:
        pts = []
        for r in records:
            xc = x_coord(r["n"])
            yc = y_coord(r[metric])
            pts.append(f"{xc:.1f},{yc:.1f}")
        svg.append(
            f'<polyline points="{" ".join(pts)}" fill="none" '
            f'stroke="{color}" stroke-width="2.5"/>'
        )
        for r in records:
            xc = x_coord(r["n"])
            yc = y_coord(r[metric])
            svg.append(
                f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="4.5" fill="{color}"/>'
            )
            svg.append(
                f'<text x="{xc:.1f}" y="{yc - 8:.1f}" fill="{color}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{r[metric]:.1f}µs</text>'
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
