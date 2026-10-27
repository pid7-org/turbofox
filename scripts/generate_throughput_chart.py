#!/usr/bin/env python3
"""Generate Catppuccin Latte KV throughput benchmark SVG"""

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

COLOR_1T = "#1e66f5" # Blue
COLOR_4T = "#8839ef" # Mauve

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)

CANDIDATE_OPERATIONS = [
    ("set_perf", "mixed_60_40_new_update", "SET", "SET (60/40 New/Update)"),
    ("set_perf", "insert", "SET", "SET (Insert)"),
    ("get_perf", "mixed_60_40", "GET", "GET (60/40 Hit/Miss)"),
    ("del_perf", "mixed_60_40", "DEL", "DEL (60/40 Hit/Miss)"),
]


def load_results(path):
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    lookup = {}
    for entry in data.get("results", []):
        bench = entry.get("benchmark")
        wl = entry.get("workload")
        thr = entry.get("threads")
        lookup[(bench, wl, thr)] = entry
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

    p_x = 0x50
    p_y = 0x55
    p_w = svg_w - 0x78
    p_h = svg_h - 0x85

    operations = [
        op for op in CANDIDATE_OPERATIONS
        if any(k[0] == op[0] and k[1] == op[1] for k in lookup)
    ]
    if not operations:
        operations = [CANDIDATE_OPERATIONS[0]]

    active_threads = sorted({
        k[2] for k in lookup
        if any(k[0] == op[0] and k[1] == op[1] for op in operations) and k[2] is not None
    })
    if not active_threads:
        active_threads = [2]

    # Find max throughput to scale Y axis
    # Limit y-axis to 6.0 kops/s (6,000 ops/sec)
    max_tp = 6000.0

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
    if len(active_threads) == 1:
        thr_label = f"{active_threads[0]} Threads"
        sub_label = (
            f"Sustained operations per second ({active_threads[0]} worker threads)"
        )
    else:
        thr_label = " vs ".join(f"{t}T" for t in active_threads)
        sub_label = "Sustained operations per second across concurrent worker threads"

    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'TurboFox Operation Throughput ({thr_label})</text>'
    )
    svg.append(
        f'<text x="32" y="48" fill="{COLOR_SUBTLE}" font-size="11">{sub_label}</text>'
    )

    # Legend
    leg_x = svg_w - 0x0100
    colors = [COLOR_1T, COLOR_4T]
    for idx, thr in enumerate(active_threads):
        c = colors[idx % len(colors)]
        x_offset = leg_x + idx * 80
        svg.append(f'<rect x="{x_offset}" y="24" width="12" height="12" rx="2" fill="{c}"/>')
        svg.append(
            f'<text x="{x_offset + 18}" y="34" fill="{COLOR_TEXT}" font-size="11">'
            f'{thr} Thread{"s" if thr > 1 else ""}</text>'
        )

    # Plot panel
    svg.append(
        f'<rect x="{p_x}" y="{p_y}" width="{p_w}" height="{p_h}" '
        f'fill="{COLOR_SURFACE}" rx="6"/>'
    )

    # Y-axis gridlines (0 to 6.0 kops/s in 1.0 kops/s intervals)
    for i in range(7):
        val = (max_tp / 6.0) * i
        y_pos = p_y + p_h - (val / max_tp) * p_h
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{format_throughput(val)}</text>'
        )

    # Grouped bars for each operation
    group_width = p_w / len(operations)
    bar_w = 0x38 # 56px per bar
    spacing = 0x0C

    for idx, (bench, wl, tag, title) in enumerate(operations):
        group_center = p_x + group_width * (idx + 0.5)

        if len(active_threads) == 1:
            thr = active_threads[0]
            e = lookup.get((bench, wl, thr), {})
            tp = e.get("throughput_ops_sec", 0.0)
            h = min((tp / max_tp) * p_h, p_h)
            y = p_y + p_h - h
            bx = group_center - (bar_w / 2)

            svg.append(
                f'<rect x="{bx:.1f}" y="{y:.1f}" width="{bar_w}" height="{h:.1f}" '
                f'rx="4" fill="{COLOR_1T}"/>'
            )
            svg.append(
                f'<text x="{bx + bar_w/2:.1f}" y="{y - 6:.1f}" fill="{COLOR_TEXT}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_throughput(tp)}</text>'
            )
            svg.append(
                f'<text x="{group_center:.1f}" y="{p_y + p_h + 20:.1f}" fill="{COLOR_TEXT}" '
                f'font-size="12" font-weight="bold" text-anchor="middle">{title}</text>'
            )
        else:
            b1_x = group_center - bar_w - (spacing / 2)
            b4_x = group_center + (spacing / 2)

            t1, t2 = active_threads[0], active_threads[1]
            e1 = lookup.get((bench, wl, t1), {})
            e4 = lookup.get((bench, wl, t2), {})
            tp1 = e1.get("throughput_ops_sec", 0.0)
            tp4 = e4.get("throughput_ops_sec", 0.0)

            h1 = min((tp1 / max_tp) * p_h, p_h)
            h4 = min((tp4 / max_tp) * p_h, p_h)

            y1 = p_y + p_h - h1
            y4 = p_y + p_h - h4

            svg.append(
                f'<rect x="{b1_x:.1f}" y="{y1:.1f}" width="{bar_w}" height="{h1:.1f}" '
                f'rx="4" fill="{COLOR_1T}"/>'
            )
            svg.append(
                f'<text x="{b1_x + bar_w/2:.1f}" y="{y1 - 6:.1f}" fill="{COLOR_TEXT}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_throughput(tp1)}</text>'
            )

            svg.append(
                f'<rect x="{b4_x:.1f}" y="{y4:.1f}" width="{bar_w}" height="{h4:.1f}" '
                f'rx="4" fill="{COLOR_4T}"/>'
            )
            svg.append(
                f'<text x="{b4_x + bar_w/2:.1f}" y="{y4 - 6:.1f}" fill="{COLOR_TEXT}" '
                f'font-size="10" font-weight="bold" text-anchor="middle">'
                f'{format_throughput(tp4)}</text>'
            )

            speedup = (tp4 / tp1) if tp1 > 0 else 1.0
            svg.append(
                f'<text x="{group_center:.1f}" y="{p_y + p_h + 20:.1f}" fill="{COLOR_TEXT}" '
                f'font-size="12" font-weight="bold" text-anchor="middle">{title}</text>'
            )
            svg.append(
                f'<text x="{group_center:.1f}" y="{p_y + p_h + 36:.1f}" fill="{COLOR_SUBTLE}" '
                f'font-size="11" text-anchor="middle">Speedup: {speedup:.2f}x</text>'
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
