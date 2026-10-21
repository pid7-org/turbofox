#!/usr/bin/env python3
"""Generate Catppuccin Latte concurrency tail latency (p99) SVG with monospace font"""

import json
import os
import sys

MEM_WORKLOADS = [
    ("del_perf", "high_contention", "High Contention"),
    ("del_perf", "delete_miss", "Delete Miss"),
    ("read_perf", "point_miss", "Point Miss"),
    ("read_perf", "point_hit", "Point Hit"),
    ("read_perf", "zipf_skewed", f"Zipfian {0x50}/{0x14}"),
    ("read_perf", "mixed_95_5", f"Mixed {0x5F}/5"),
    ("del_perf", "delete_hit", "Delete Hit"),
    ("del_perf", "churn_insert_delete", "Churn 50/50"),
]

DISK_WORKLOADS = [
    ("write_ingest", "sequential_append", "Seq Ingest"),
]

THREADS = [1, 2, 4]
THREAD_COLORS = ["#1e66f5", "#40a02b", "#fe640b"]

FONT_STACK = (
    "'JetBrains Mono','Fira Code','Roboto Mono',Menlo,Monaco,Consolas,monospace"
)


def load_results(path):
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    lookup = {}
    for entry in data.get("results", []):
        bench = entry["benchmark"]
        workload = entry["workload"]
        threads = entry["threads"]
        lookup[(bench, workload, threads)] = entry
        if bench == "write_perf" and workload == "pure_append":
            lookup[("write_ingest", "sequential_append", threads)] = entry
    return lookup


def generate_svg(lookup):
    svg_w = 0x0460
    svg_h = 0x0244
    p1_x = 0x8C
    p1_y = 0x55
    p1_w = 0x0250
    p1_h = 0x01B0

    p2_x = 0x035C
    p2_y = p1_y
    p2_w = 0xD0
    p2_h = p1_h

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:#eff1f5; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="#eff1f5" stroke="#ccd0da" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{p1_x}" y="{0x22}" fill="#4c4f69" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Concurrency Tail Latency (p99 Profile)'
        f'</text>'
    )
    svg.append(
        f'<text x="{p1_x}" y="{0x38}" fill="#6c6f85" font-size="{0x0A}">'
        f'p99 tail latency degradation (µs) across 1, 2, and 4 threads'
        f'</text>'
    )

    # Panel 1: In-Memory & Churn (0 to 22 µs)
    max_p1 = 22.0
    for tick in [0, 5, 10, 15, 20]:
        x = p1_x + (tick / max_p1) * p1_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{p1_y}" x2="{x:.1f}" '
            f'y2="{p1_y + p1_h}" stroke="#dce0e8" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{p1_y + p1_h + 0x12}" fill="#8c8fa1" '
            f'font-size="{0x09}" text-anchor="middle">{tick} µs</text>'
        )

    row_h1 = p1_h / len(MEM_WORKLOADS)
    bar_h = 0x07
    for r_idx, (bench, workload, label) in enumerate(MEM_WORKLOADS):
        y_top = p1_y + r_idx * row_h1
        svg.append(
            f'<text x="{p1_x - 0x0A}" y="{y_top + row_h1 / 2.0 + 3:.1f}" '
            f'fill="#4c4f69" font-size="{0x09}" font-weight="600" '
            f'text-anchor="end">{label}</text>'
        )
        for t_idx, thr in enumerate(THREADS):
            entry = lookup.get((bench, workload, thr))
            val = entry["latency_micros"]["p99"] if entry else 0.0
            bw = max((val / max_p1) * p1_w, 2.0)
            by = y_top + 2 + t_idx * (bar_h + 2)
            color = THREAD_COLORS[t_idx]
            svg.append(
                f'<rect x="{p1_x}" y="{by:.1f}" width="{bw:.1f}" '
                f'height="{bar_h}" rx="2" fill="{color}"/>'
            )
            svg.append(
                f'<text x="{p1_x + bw + 4:.1f}" y="{by + 6:.1f}" fill="{color}" '
                f'font-size="{0x07}" font-weight="600">{val:.2f}</text>'
            )

    # Panel 2: Disk Seq Ingest (0 to 10,000 µs = 10 ms)
    max_p2 = 10000.0
    for tick in [0, 5000, 10000]:
        x = p2_x + (tick / max_p2) * p2_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{p2_y}" x2="{x:.1f}" '
            f'y2="{p2_y + p2_h}" stroke="#dce0e8" stroke-width="1"/>'
        )
        ms_lbl = f"{tick / 1000.0:.0f} ms" if tick > 0 else "0"
        svg.append(
            f'<text x="{x:.1f}" y="{p2_y + p2_h + 0x12}" fill="#8c8fa1" '
            f'font-size="{0x09}" text-anchor="middle">{ms_lbl}</text>'
        )

    y_top2 = p2_y + 0x30
    svg.append(
        f'<text x="{p2_x}" y="{y_top2 - 0x0A}" fill="#4c4f69" '
        f'font-size="{0x0A}" font-weight="bold">Disk Persistence (WAL)</text>'
    )
    for t_idx, thr in enumerate(THREADS):
        entry = lookup.get(("write_ingest", "sequential_append", thr))
        val = entry["latency_micros"]["p99"] if entry else 0.0
        bw = max((val / max_p2) * p2_w, 2.0)
        by = y_top2 + t_idx * 0x14
        color = THREAD_COLORS[t_idx]
        svg.append(
            f'<rect x="{p2_x}" y="{by:.1f}" width="{bw:.1f}" '
            f'height="{0x0B}" rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{p2_x + bw + 4:.1f}" y="{by + 9:.1f}" fill="{color}" '
            f'font-size="{0x08}" font-weight="600">{val / 1000.0:.2f} ms</text>'
        )

    # Legend
    leg_x = svg_w - 0xC8
    leg_y = 0x22
    for t_idx, thr in enumerate(THREADS):
        lx = leg_x + t_idx * 0x36
        svg.append(
            f'<rect x="{lx}" y="{leg_y - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{THREAD_COLORS[t_idx]}"/>'
        )
        svg.append(
            f'<text x="{lx + 0x0E}" y="{leg_y}" fill="#4c4f69" '
            f'font-size="{0x09}" font-weight="600">{thr}T</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    json_path = os.path.join(root, "tmp", "turbofox_benchmark_results.json")
    out_path = os.path.join(root, "assets", "concurrency_tail_latency.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
