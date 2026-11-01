#!/usr/bin/env python3

import json
import os
import sys

COLOR_BASE = "#fff"        # Plain white background
COLOR_SURFACE = "#f8f9fa"   # Light neutral plot area surface
COLOR_BORDER = "#d0d7de"    # Container border
COLOR_LINE = "#e1e4e8"      # Grid line
COLOR_TEXT = "#24292f"      # Primary text
COLOR_SUBTLE = "#57606a"    # Secondary text and axis ticks

OPERATION_COLORS = {
    "SET": "#1e66f5", # Blue
    "GET": "#179299", # Teal
    "DEL": "#8839ef", # Mauve
}
COLOR_FALLBACK = "#1e66f5"

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)

CANDIDATE_OPERATIONS = [
    ("set_perf", "mixed_80_20_new_update", "SET", "SET (80/20 New/Update)"),
    ("set_perf", "mixed_60_40_new_update", "SET", "SET (60/40 New/Update)"),
    ("set_perf", "insert", "SET", "SET (Insert)"),
    ("get_perf", "mixed_60_40", "GET", "GET (60/40 Hit/Miss)"),
    ("del_perf", "mixed_60_40", "DEL", "DEL (60/40 Hit/Miss)"),
]


def load_results(path):
    with open(path, "r", encoding="utf-8") as handle:
        raw = handle.read().strip()
    if not raw:
        return {}

    # Support raw comma-separated JSON objects as emitted by kv_perf benchmark
    if not raw.startswith("[") and not (raw.startswith("{") and '"results"' in raw):
        raw = f"[{raw}]"

    data = json.loads(raw)
    entries = data.get("results", data) if isinstance(data, dict) else data
    lookup = {}
    for entry in entries:
        if isinstance(entry, dict):
            bench = entry.get("benchmark")
            wl = entry.get("workload")
            if (bench, wl) not in lookup:
                lookup[(bench, wl)] = entry
    return lookup


def format_throughput(ops_sec):
    if ops_sec >= 1_000_000:
        return f"{ops_sec / 1_000_000:.2f} Mops/s"
    elif ops_sec >= 1_000:
        return f"{ops_sec / 1_000:.1f} kops/s"
    else:
        return f"{ops_sec:.0f} ops/s"


def generate_svg(lookup, out_path):
    svg_w = 0x03E8 # 1000 px
    svg_h = 0x01E0 # 480 px

    p_x = 0x6E # 110 px: provides clearance for y-axis labels
    p_y = 0x55 # 85 px
    p_w = svg_w - p_x - 0x28 # 850 px
    p_h = svg_h - 0x85 # 347 px

    operations = [
        op for op in CANDIDATE_OPERATIONS
        if (op[0], op[1]) in lookup
    ]
    if not operations:
        operations = [CANDIDATE_OPERATIONS[0]]

    # Limit y-axis to 1.2 Mops/s (1,200,000 ops/sec)
    max_tp = 1_200_000.0

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
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Operation Throughput</text>'
    )
    svg.append(
        f'<text x="32" y="48" fill="{COLOR_SUBTLE}" font-size="11">'
        f'Sustained operations per second across core key-value operations</text>'
    )

    # Plot panel
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6"/>'
    )

    # Y-axis gridlines (0 to 1.2 Mops/s in 0.2 Mops/s intervals)
    for i in range(7):
        val = (max_tp / 6.0) * i
        y_pos = p_y + p_h - (val / max_tp) * p_h
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 12}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{format_throughput(val)}</text>'
        )

    # Center single bars for each operation
    group_width = p_w / len(operations)
    bar_w = 0x48 # 72px per bar

    for idx, (bench, wl, tag, title) in enumerate(operations):
        bar_color = OPERATION_COLORS.get(tag, COLOR_FALLBACK)
        group_center = p_x + group_width * (idx + 0.5)
        e = lookup.get((bench, wl), {})
        tp = e.get("throughput_ops_sec", 0.0)
        h = min((tp / max_tp) * p_h, p_h)
        y = p_y + p_h - h
        bx = group_center - (bar_w / 2)

        svg.append(
            f'<rect x="{bx:.1f}" y="{y:.1f}" width="{bar_w}" height="{h:.1f}" '
            f'rx="4" fill="{bar_color}"/>'
        )
        svg.append(
            f'<text x="{group_center:.1f}" y="{y - 6:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="10" font-weight="bold" text-anchor="middle">'
            f'{format_throughput(tp)}</text>'
        )
        svg.append(
            f'<text x="{group_center:.1f}" y="{p_y + p_h + 20:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="12" font-weight="bold" text-anchor="middle">{title}</text>'
        )

    svg.append('</svg>')

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(svg))


if __name__ == "__main__":
    in_file = sys.argv[1] if len(sys.argv) > 1 else "tmp/results/turbofox_benchmark_results.json"
    out_file = sys.argv[2] if len(sys.argv) > 2 else "assets/kv_throughput_scaling.svg"
    data = load_results(in_file)
    generate_svg(data, out_file)
