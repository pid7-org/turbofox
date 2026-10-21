#!/usr/bin/env python3
"""Generate Rosé Pine Dawn multi-thread speedup SVG with monospace font"""

import json
import os
import sys
import xml.etree.ElementTree as ET

# Rosé Pine Dawn palette
COLOR_BASE = "#faf4ed"
COLOR_SURFACE = "#fffaf3"
COLOR_OVERLAY = "#f2e9e1"
COLOR_MUTED = "#9893a5"
COLOR_SUBTLE = "#797593"
COLOR_TEXT = "#575279"
COLOR_BORDER = "#dfdad9"
COLOR_LINE = "#cecacd"

# Accent colors for threads (Pine, Iris, Love)
THREAD_COLORS = ["#286983", "#907aa9", "#b4637a"]
THREADS = [1, 2, 4]

# Workloads profiled: (bench, workload, display_label)
WORKLOADS = [
    ("del_perf", "churn_insert_delete", f"CHURN: {0x32}/{0x32} Put/Del"),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)"),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)"),
    ("read_perf", "point_hit", "READ: Point Hit (Existing)"),
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention"),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)"),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)"),
    ("read_perf", "mixed_95_5", f"READ/WRITE: Mixed {0x5F}/5 OLTP"),
    ("write_ingest", "sequential_append", "WAL: Ingest (NVMe fsync)"),
]

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
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
    svg_w = 0x04C4
    svg_h = 0x0218
    title_x = 0x24
    plot_x = 0x0118
    plot_y = 0x50
    plot_w = 0x0350
    plot_h = 0x0140

    max_scale = 2.0
    num_rows = len(WORKLOADS)
    row_h = float(plot_h) / float(num_rows)
    bar_h = 0x07

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Multi-Thread Speedup Ratio (c5d.large - 2 vCPUs)'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" font-size="{0x0A}">'
        f'Relative throughput scaling (Ops_N / Ops_1T) across worker threads '
        f'(Ros\u00e9 Pine Dawn theme)'
        f'</text>'
    )

    # Legend in header
    leg_x = svg_w - 0x010E
    leg_y = 0x22
    for t_idx, thr in enumerate(THREADS):
        lx = leg_x + t_idx * 0x36
        svg.append(
            f'<rect x="{lx}" y="{leg_y - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{THREAD_COLORS[t_idx]}"/>'
        )
        svg.append(
            f'<text x="{lx + 0x0E}" y="{leg_y}" fill="{COLOR_TEXT}" '
            f'font-size="{0x09}" font-weight="bold">{thr}T</text>'
        )

    # Gridlines and numeric ticks
    ticks = [0.0, 0.5, 1.0, 1.5, 2.0]
    for tick in ticks:
        x = plot_x + (tick / max_scale) * plot_w
        dash = ' stroke-dasharray="3,3"' if tick == 2.0 else ""
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"{dash}/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{tick:.1f}x</text>'
        )

    # Axis boundary line
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    # Workload rows
    for r_idx, (bench, workload, label) in enumerate(WORKLOADS):
        base_entry = lookup.get((bench, workload, 1))
        base_ops = base_entry["throughput_ops_sec"] if base_entry else 1.0
        y_top = plot_y + float(r_idx) * row_h

        svg.append(
            f'<text x="{plot_x - 0x0A}" y="{y_top + row_h / 2.0 + 3:.1f}" '
            f'fill="{COLOR_TEXT}" font-size="{0x08}" font-weight="bold" '
            f'text-anchor="end">{label}</text>'
        )

        for t_idx, thr in enumerate(THREADS):
            cur_entry = lookup.get((bench, workload, thr))
            cur_ops = cur_entry["throughput_ops_sec"] if cur_entry else base_ops
            ratio = cur_ops / base_ops
            bar_w = max((ratio / max_scale) * plot_w, 2.0)
            by = y_top + 2 + t_idx * (bar_h + 2)
            color = THREAD_COLORS[t_idx]

            svg.append(
                f'<rect x="{plot_x}" y="{by:.1f}" width="{bar_w:.1f}" '
                f'height="{bar_h}" rx="2" fill="{color}"/>'
            )
            svg.append(
                f'<text x="{plot_x + bar_w + 4:.1f}" y="{by + 6:.1f}" '
                f'fill="{color}" font-size="{0x07}" font-weight="bold">'
                f'{ratio:.2f}x</text>'
            )

    # Minimal left-aligned side notes without container box
    note_x = title_x
    note_y = plot_y + plot_h + 0x2A
    note_lines = [
        "* Note: Speedup ratio (Ops_N / Ops_1T) benchmarked on AWS c5d.large (2 physical vCPUs); "
        "2.0x represents ideal linear scaling limit",
        "  In-memory read and churn workloads scale to ~1.22x - 1.32x at 2T saturation "
        "before hyperthread cache contention plateaus scaling at 4T",
        "  WAL Ingest exhibits flat-to-regressive scaling (~0.85x - 1.03x) constrained by "
        "serialize-on-fsync queueing and storage controller barriers",
    ]
    for idx, line in enumerate(note_lines):
        ly = note_y + idx * 0x10
        svg.append(
            f'<text x="{note_x}" y="{ly}" fill="{COLOR_SUBTLE}" '
            f'font-size="{0x08}">{line}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    json_path = os.path.join(root, "tmp", "turbofox_benchmark_results.json")
    out_path = os.path.join(root, "assets", "multithread_speedup.svg")

    if not os.path.exists(json_path):
        sys.stderr.write(f"benchmark results missing at {json_path}\n")
        sys.exit(1)

    lookup = load_results(json_path)
    svg = generate_svg(lookup)

    # Validate XML before writing
    ET.fromstring(svg)

    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
        handle.write("\n")
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
