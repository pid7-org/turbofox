#!/usr/bin/env python3
"""Generate Catppuccin Latte latency jitter & variance SVG with monospace font"""

import json
import os
import sys

WORKLOADS = [
    ("write_ingest", "sequential_append", "Seq Ingest (WAL)", f"#{0xD20F39:06x}"),
    ("del_perf", "high_contention", "High Contention", f"#{0x8839EF:06x}"),
    ("read_perf", "point_miss", "Point Miss", f"#{0x04A5E5:06x}"),
    ("read_perf", "mixed_95_5", f"Mixed {0x5F}/5", f"#{0xFE640B:06x}"),
    ("del_perf", "churn_insert_delete", "Churn 50/50", f"#{0xEA76CB:06x}"),
    ("read_perf", "point_hit", "Point Hit", f"#{0x1E66F5:06x}"),
    ("del_perf", "delete_miss", "Delete Miss", f"#{0x179299:06x}"),
    ("read_perf", "zipf_skewed", f"Zipfian {0x50}/{0x14}", f"#{0x7287FD:06x}"),
    ("del_perf", "delete_hit", "Delete Hit", f"#{0x40A02B:06x}"),
]

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
    svg_w = 0x0460
    svg_h = 0x0244
    plot_x = 0x0100
    plot_y = 0x55
    plot_w = 0x0320
    plot_h = 0x01B0

    max_cv = 8.0
    row_h = plot_h / len(WORKLOADS)
    bar_h = 0x0E

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
        f'<text x="{0x30}" y="{0x22}" fill="#4c4f69" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Latency Predictability &amp; Jitter Profile (CV = \u03c3 / \u03bc)'
        f'</text>'
    )
    svg.append(
        f'<text x="{0x30}" y="{0x38}" fill="#6c6f85" font-size="{0x0A}">'
        f'Coefficient of variation (1T baseline): lower indicates higher SLA predictability'
        f'</text>'
    )

    # Gridlines
    ticks = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
    for tick in ticks:
        x = plot_x + (tick / max_cv) * plot_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="#dce0e8" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="#8c8fa1" '
            f'font-size="{0x09}" text-anchor="middle">{tick:.1f}</text>'
        )

    # Workload rows
    for idx, (bench, workload, label, color) in enumerate(WORKLOADS):
        entry = lookup.get((bench, workload, 1))
        lat = entry.get("latency_micros", {}) if entry else {}
        mean_val = lat.get("mean", 1.0)
        std_val = lat.get("stddev", 0.0)
        cv = std_val / mean_val if mean_val > 0 else 0.0
        bw = max((cv / max_cv) * plot_w, 2.0)
        by = plot_y + idx * row_h + (row_h - bar_h) / 2.0

        svg.append(
            f'<text x="{plot_x - 0x0A}" y="{by + 11:.1f}" fill="#4c4f69" '
            f'font-size="{0x09}" font-weight="600" text-anchor="end">{label}</text>'
        )
        svg.append(
            f'<rect x="{plot_x}" y="{by:.1f}" width="{bw:.1f}" '
            f'height="{bar_h}" rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{plot_x + bw + 6:.1f}" y="{by + 11:.1f}" fill="{color}" '
            f'font-size="{0x08}" font-weight="600">'
            f'CV={cv:.2f} (\u03bc={mean_val:.2f}\u00b5s, \u03c3={std_val:.2f}\u00b5s)</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    json_path = os.path.join(root, "tmp", "turbofox_benchmark_results.json")
    out_path = os.path.join(root, "assets", "latency_jitter.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
