#!/usr/bin/env python3
"""Generate Catppuccin Latte KV tail latency benchmark SVG"""

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

CANDIDATE_WORKLOADS = [
    ("set_perf", "mixed_60_40_new_update", "SET: 60/40 New/Update"),
    ("set_perf", "insert", "SET: Insert"),
    ("get_perf", "mixed_60_40", "GET: 60/40 Hit/Miss"),
    ("del_perf", "mixed_60_40", "DEL: 60/40 Hit/Miss"),
]

PERCENTILES = ["p50", "p90", "p99"]
PERCENTILE_LABELS = {"p50": "p50", "p90": "p90", "p99": "p99"}


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


def generate_svg(lookup, out_path):
    svg_w = 0x044C # 1100 px
    svg_h = 0x021C # 540 px

    p_x = 0x50
    p_y = 0x55
    p_w = svg_w - 0x78
    p_h = svg_h - 0x85

    workloads = [
        wl for wl in CANDIDATE_WORKLOADS
        if any(k[0] == wl[0] and k[1] == wl[1] for k in lookup)
    ]
    if not workloads:
        workloads = [CANDIDATE_WORKLOADS[0]]

    active_threads = sorted({
        k[2] for k in lookup
        if any(k[0] == wl[0] and k[1] == wl[1] for wl in workloads) and k[2] is not None
    })
    if not active_threads:
        active_threads = [2]

    # Limit y-axis to 2ms (2,000 µs) to prevent outliers from skewing chart
    max_lat = 2000.0

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
            f"Tail latencies in microseconds across percentiles ({active_threads[0]} worker threads)"
        )
    else:
        thr_label = " vs ".join(f"{t}T" for t in active_threads)
        sub_label = "Tail latencies in microseconds across p50, p90, and p99"

    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'TurboFox Operation Tail Latency ({thr_label})</text>'
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

    # Grid lines (0 to 2000 µs in 500 µs intervals)
    for i in range(5):
        val = (max_lat / 4.0) * i
        y_pos = p_y + p_h - (val / max_lat) * p_h
        svg.append(
            f'<line x1="{p_x}" y1="{y_pos:.1f}" x2="{p_x + p_w}" y2="{y_pos:.1f}" '
            f'stroke="{COLOR_LINE}" stroke-dasharray="3,3" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{p_x - 10}" y="{y_pos + 4:.1f}" fill="{COLOR_SUBTLE}" '
            f'font-size="10" text-anchor="end">{val:.0f}µs</text>'
        )

    # Layout: workload blocks
    graph_spacing = 24.0
    block_w = (p_w - (len(workloads) - 1) * graph_spacing) / len(workloads)
    bar_w = 0x12 # 18px
    bar_spacing = 0x04

    for w_idx, (bench, wl, title) in enumerate(workloads):
        block_left = p_x + w_idx * (block_w + graph_spacing)

        p_sub_w = block_w / len(PERCENTILES)
        for p_idx, p_key in enumerate(PERCENTILES):
            center = block_left + p_sub_w * (p_idx + 0.5)

            if len(active_threads) == 1:
                thr = active_threads[0]
                e = lookup.get((bench, wl, thr), {}).get("latency_micros", {})
                v = e.get(p_key, 0.0)
                h = min((v / max_lat) * p_h, p_h)
                y = p_y + p_h - h
                bx = center - bar_w / 2

                svg.append(
                    f'<rect x="{bx:.1f}" y="{y:.1f}" width="{bar_w}" height="{h:.1f}" '
                    f'rx="3" fill="{COLOR_1T}"/>'
                )
                txt_y = y - 4 if h < p_h - 20 else y + 12
                txt_color = COLOR_TEXT
                svg.append(
                    f'<text x="{center:.1f}" y="{txt_y:.1f}" fill="{txt_color}" '
                    f'font-size="9" font-weight="bold" text-anchor="middle">{v:.0f}µs</text>'
                )
            else:
                t1, t2 = active_threads[0], active_threads[1]
                e1 = lookup.get((bench, wl, t1), {}).get("latency_micros", {})
                e4 = lookup.get((bench, wl, t2), {}).get("latency_micros", {})

                b1_x = center - bar_w - (bar_spacing / 2)
                b4_x = center + (bar_spacing / 2)

                v1 = e1.get(p_key, 0.0)
                v4 = e4.get(p_key, 0.0)

                h1 = min((v1 / max_lat) * p_h, p_h)
                h4 = min((v4 / max_lat) * p_h, p_h)

                y1 = p_y + p_h - h1
                y4 = p_y + p_h - h4

                svg.append(
                    f'<rect x="{b1_x:.1f}" y="{y1:.1f}" width="{bar_w}" height="{h1:.1f}" '
                    f'rx="3" fill="{COLOR_1T}"/>'
                )
                svg.append(
                    f'<rect x="{b4_x:.1f}" y="{y4:.1f}" width="{bar_w}" height="{h4:.1f}" '
                    f'rx="3" fill="{COLOR_4T}"/>'
                )

                txt1_y = y1 - 4 if h1 < p_h - 20 else y1 + 12
                txt1_c = COLOR_TEXT
                svg.append(
                    f'<text x="{b1_x + bar_w/2:.1f}" y="{txt1_y:.1f}" fill="{txt1_c}" '
                    f'font-size="8" text-anchor="middle">{v1:.0f}µs</text>'
                )

                txt4_y = y4 - 4 if h4 < p_h - 20 else y4 + 12
                txt4_c = COLOR_TEXT
                svg.append(
                    f'<text x="{b4_x + bar_w/2:.1f}" y="{txt4_y:.1f}" fill="{txt4_c}" '
                    f'font-size="8" text-anchor="middle">{v4:.0f}µs</text>'
                )

            svg.append(
                f'<text x="{center:.1f}" y="{p_y + p_h + 16}" fill="{COLOR_SUBTLE}" '
                f'font-size="10" text-anchor="middle">{PERCENTILE_LABELS[p_key]}</text>'
            )

        svg.append(
            f'<text x="{block_left + block_w / 2:.1f}" y="{p_y + p_h + 34}" '
            f'fill="{COLOR_TEXT}" font-size="12" font-weight="bold" text-anchor="middle">'
            f'{title}</text>'
        )

    svg.append('</svg>')

    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write("\n".join(svg))


if __name__ == "__main__":
    in_file = sys.argv[1] if len(sys.argv) > 1 else "tmp/results/turbofox_benchmark_results.json"
    out_file = sys.argv[2] if len(sys.argv) > 2 else "assets/kv_tail_latency.svg"
    data = load_results(in_file)
    generate_svg(data, out_file)
