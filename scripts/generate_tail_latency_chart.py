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

COLOR_1T = "#1e66f5" # Blue
COLOR_2T = "#8839ef" # Mauve
COLOR_4T = COLOR_2T

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)

CANDIDATE_WORKLOADS = [
    ("set_perf", "mixed_80_20_new_update", "SET: 80/20 New/Update"),
    ("set_perf", "mixed_60_40_new_update", "SET: 60/40 New/Update"),
    ("set_perf", "insert", "SET: Insert"),
    ("get_perf", "mixed_60_40", "GET: 60/40 Hit/Miss"),
    ("del_perf", "mixed_60_40", "DEL: 60/40 Hit/Miss"),
]

PERCENTILES = ["p50", "p90", "p99", "p99.99"]
PERCENTILE_LABELS = {
    "p50": "p50",
    "p90": "p90",
    "p99": "p99",
    "p99.99": "p9999",
}


def get_latency_val(lat_map, p_key):
    if not isinstance(lat_map, dict):
        return 0.0
    if p_key in lat_map:
        return lat_map[p_key]
    if p_key == "p99.99":
        for k in ("p999", "p99.99", "p9999", "p99.9"):
            if k in lat_map:
                return lat_map[k]
    return 0.0


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

    # Limit y-axis to 100 µs with 5 intervals
    max_lat = 100.0
    intervals = 5

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
        sub_label = "Tail latencies in microseconds across p50, p90, p99, and p9999"

    svg.append(
        f'<text x="32" y="32" fill="{COLOR_TEXT}" font-size="16" font-weight="bold">'
        f'Operation Tail Latency ({thr_label})</text>'
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

    # Grid lines (0 to 100 µs in 5 intervals: 0, 20, 40, 60, 80, 100 µs)
    for i in range(intervals + 1):
        val = (max_lat / float(intervals)) * i
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
    bar_w = 0x10 if len(workloads) <= 3 else 0x0E # 16px / 14px
    bar_spacing = 0x04

    for w_idx, (bench, wl, title) in enumerate(workloads):
        block_left = p_x + w_idx * (block_w + graph_spacing)

        p_sub_w = block_w / len(PERCENTILES)
        for p_idx, p_key in enumerate(PERCENTILES):
            center = block_left + p_sub_w * (p_idx + 0.5)

            if len(active_threads) == 1:
                thr = active_threads[0]
                e = lookup.get((bench, wl, thr), {}).get("latency_micros", {})
                v = get_latency_val(e, p_key)
                overflow = v > max_lat
                h = min((v / max_lat) * p_h, p_h)
                y = p_y + p_h - h
                bx = center - bar_w / 2

                svg.append(
                    f'<rect x="{bx:.1f}" y="{y:.1f}" width="{bar_w}" height="{h:.1f}" '
                    f'rx="3" fill="{COLOR_1T}"/>'
                )
                if overflow:
                    svg.append(
                        f'<line x1="{bx:.1f}" y1="{p_y:.1f}" x2="{bx + bar_w:.1f}" y2="{p_y:.1f}" '
                        f'stroke="#d20f39" stroke-width="2"/>'
                    )

                txt = f"{v:.0f}µs"
                if overflow or h >= p_h - 24:
                    bw = max(len(txt) * 5.8 + 8.0, 30.0)
                    ty = p_y + 14
                    svg.append(
                        f'<rect x="{center - bw/2:.1f}" y="{ty - 10:.1f}" '
                        f'width="{bw:.1f}" height="14" rx="3" '
                        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
                    )
                    svg.append(
                        f'<text x="{center:.1f}" y="{ty:.1f}" fill="{COLOR_1T}" '
                        f'font-size="8" font-weight="bold" text-anchor="middle">{txt}</text>'
                    )
                else:
                    svg.append(
                        f'<text x="{center:.1f}" y="{y - 4:.1f}" fill="{COLOR_TEXT}" '
                        f'font-size="8" text-anchor="middle">{txt}</text>'
                    )
            else:
                t1, t2 = active_threads[0], active_threads[1]
                e1 = lookup.get((bench, wl, t1), {}).get("latency_micros", {})
                e4 = lookup.get((bench, wl, t2), {}).get("latency_micros", {})

                b1_x = center - bar_w - (bar_spacing / 2)
                b4_x = center + (bar_spacing / 2)

                v1 = get_latency_val(e1, p_key)
                v4 = get_latency_val(e4, p_key)

                overflow1 = v1 > max_lat
                overflow4 = v4 > max_lat

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

                if overflow1:
                    svg.append(
                        f'<line x1="{b1_x:.1f}" y1="{p_y:.1f}" x2="{b1_x + bar_w:.1f}" y2="{p_y:.1f}" '
                        f'stroke="#d20f39" stroke-width="2"/>'
                    )
                if overflow4:
                    svg.append(
                        f'<line x1="{b4_x:.1f}" y1="{p_y:.1f}" x2="{b4_x + bar_w:.1f}" y2="{p_y:.1f}" '
                        f'stroke="#d20f39" stroke-width="2"/>'
                    )

                txt1 = f"{v1:.0f}µs"
                txt4 = f"{v4:.0f}µs"

                near_top1 = overflow1 or h1 >= p_h - 24
                near_top4 = overflow4 or h4 >= p_h - 24

                c1 = b1_x + bar_w / 2
                c4 = b4_x + bar_w / 2

                if near_top1 and near_top4:
                    bw1 = max(len(txt1) * 5.8 + 8.0, 30.0)
                    bw4 = max(len(txt4) * 5.8 + 8.0, 30.0)
                    ty1 = p_y + 14
                    ty4 = p_y + 30

                    svg.append(
                        f'<rect x="{c1 - bw1/2:.1f}" y="{ty1 - 10:.1f}" '
                        f'width="{bw1:.1f}" height="14" rx="3" '
                        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
                    )
                    svg.append(
                        f'<text x="{c1:.1f}" y="{ty1:.1f}" fill="{COLOR_1T}" '
                        f'font-size="8" font-weight="bold" text-anchor="middle">{txt1}</text>'
                    )

                    svg.append(
                        f'<rect x="{c4 - bw4/2:.1f}" y="{ty4 - 10:.1f}" '
                        f'width="{bw4:.1f}" height="14" rx="3" '
                        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
                    )
                    svg.append(
                        f'<text x="{c4:.1f}" y="{ty4:.1f}" fill="{COLOR_4T}" '
                        f'font-size="8" font-weight="bold" text-anchor="middle">{txt4}</text>'
                    )
                else:
                    if near_top1:
                        bw1 = max(len(txt1) * 5.8 + 8.0, 30.0)
                        ty1 = p_y + 14
                        svg.append(
                            f'<rect x="{c1 - bw1/2:.1f}" y="{ty1 - 10:.1f}" '
                            f'width="{bw1:.1f}" height="14" rx="3" '
                            f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
                        )
                        svg.append(
                            f'<text x="{c1:.1f}" y="{ty1:.1f}" fill="{COLOR_1T}" '
                            f'font-size="8" font-weight="bold" text-anchor="middle">{txt1}</text>'
                        )
                    else:
                        svg.append(
                            f'<text x="{c1:.1f}" y="{y1 - 4:.1f}" fill="{COLOR_TEXT}" '
                            f'font-size="8" text-anchor="middle">{txt1}</text>'
                        )

                    if near_top4:
                        bw4 = max(len(txt4) * 5.8 + 8.0, 30.0)
                        ty4 = p_y + 14
                        svg.append(
                            f'<rect x="{c4 - bw4/2:.1f}" y="{ty4 - 10:.1f}" '
                            f'width="{bw4:.1f}" height="14" rx="3" '
                            f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
                        )
                        svg.append(
                            f'<text x="{c4:.1f}" y="{ty4:.1f}" fill="{COLOR_4T}" '
                            f'font-size="8" font-weight="bold" text-anchor="middle">{txt4}</text>'
                        )
                    else:
                        svg.append(
                            f'<text x="{c4:.1f}" y="{y4 - 4:.1f}" fill="{COLOR_TEXT}" '
                            f'font-size="8" text-anchor="middle">{txt4}</text>'
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
