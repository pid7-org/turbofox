#!/usr/bin/env python3
"""Generate multi-thread speedup and scaling efficiency SVG with monospace font"""

import json
import os
import sys

WORKLOADS = [
    ("del_perf", "churn_insert_delete", "Churn 50/50"),
    ("del_perf", "delete_hit", "Delete Hit"),
    ("read_perf", "zipf_skewed", f"Zipfian {0x50}/{0x14}"),
    ("read_perf", "point_hit", "Point Hit"),
    ("del_perf", "high_contention", "High Contention"),
    ("del_perf", "delete_miss", "Delete Miss"),
    ("read_perf", "point_miss", "Point Miss"),
    ("read_perf", "mixed_95_5", f"Mixed {0x5F}/5"),
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
    return lookup


def generate_svg(lookup):
    svg_w = 0x0440
    svg_h = 0x0270
    plot_x = 0x8C
    plot_y = 0x55
    plot_w = 0x0370
    plot_h = 0x01E0

    # Max speedup scale up to 2.5x (c5d.large has 2 vCPUs)
    max_scale = 2.0
    row_h = plot_h / len(WORKLOADS)
    bar_h = 0x09

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
        f'<text x="{plot_x}" y="{0x22}" fill="#4c4f69" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Multi-Thread Speedup Ratio (c5d.large - 2 vCPUs)'
        f'</text>'
    )
    svg.append(
        f'<text x="{plot_x}" y="{0x38}" fill="#6c6f85" font-size="{0x0A}">'
        f'Relative throughput scaling (Ops_N / Ops_1T) across worker threads'
        f'</text>'
    )

    # Gridlines
    ticks = [0.0, 0.5, 1.0, 1.5, 2.0]
    for tick in ticks:
        x = plot_x + (tick / max_scale) * plot_w
        dash = ' stroke-dasharray="3,3"' if tick == 2.0 else ""
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="#dce0e8" stroke-width="1"{dash}/>'
        )
        label = f"{tick:.1f}x"
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="#8c8fa1" '
            f'font-size="{0x09}" text-anchor="middle">{label}</text>'
        )

    # Workload rows
    for r_idx, (bench, workload, label) in enumerate(WORKLOADS):
        base_entry = lookup.get((bench, workload, 1))
        base_ops = base_entry["throughput_ops_sec"] if base_entry else 1.0
        y_top = plot_y + r_idx * row_h

        svg.append(
            f'<text x="{plot_x - 0x0A}" y="{y_top + row_h / 2.0 + 4:.1f}" '
            f'fill="#4c4f69" font-size="{0x0A}" font-weight="600" '
            f'text-anchor="end">{label}</text>'
        )

        for t_idx, thr in enumerate(THREADS):
            cur_entry = lookup.get((bench, workload, thr))
            cur_ops = cur_entry["throughput_ops_sec"] if cur_entry else base_ops
            ratio = cur_ops / base_ops
            bar_w = max((ratio / max_scale) * plot_w, 2.0)
            by = y_top + 4 + t_idx * (bar_h + 2)
            color = THREAD_COLORS[t_idx]

            svg.append(
                f'<rect x="{plot_x}" y="{by:.1f}" width="{bar_w:.1f}" '
                f'height="{bar_h}" rx="2" fill="{color}"/>'
            )
            svg.append(
                f'<text x="{plot_x + bar_w + 4:.1f}" y="{by + 7.5:.1f}" '
                f'fill="{color}" font-size="{0x08}" font-weight="600">'
                f'{ratio:.2f}x</text>'
            )

    # Legend at top right
    leg_x = plot_x + plot_w - 0xA0
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
    out_path = os.path.join(root, "assets", "multithread_speedup.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
