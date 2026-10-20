#!/usr/bin/env python3
"""Generate NVMe persistence latency distribution SVG with monospace font"""

import json
import math
import os
import sys

SERIES = [
    ("write_ingest", "sequential_append", 1, "Seq Ingest (1T)", f"#{0x1E66F5:06x}"),
    ("write_ingest", "sequential_append", 2, "Seq Ingest (2T)", f"#{0x40A02B:06x}"),
    ("write_ingest", "sequential_append", 4, "Seq Ingest (4T)", f"#{0xFE640B:06x}"),
    ("del_perf", "churn_insert_delete", 1, "Churn 50/50 (1T)", f"#{0x8839EF:06x}"),
    ("del_perf", "churn_insert_delete", 2, "Churn 50/50 (2T)", f"#{0xEA76CB:06x}"),
    ("del_perf", "churn_insert_delete", 4, "Churn 50/50 (4T)", f"#{0xD20F39:06x}"),
]

PERCENTILES = [
    ("min", "min"),
    ("p50", "p50"),
    ("p75", "p75"),
    ("p90", "p90"),
    ("p95", "p95"),
    ("p99", "p99"),
    ("p999", "p99.9"),
    ("max", "max"),
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
    svg_w = 0x0400
    svg_h = 0x0230
    plot_x = 0x50
    plot_y = 0x58
    plot_w = 0x0280
    plot_h = 0x0160

    # Log scale: 10^0 (1 µs) to 10^5 (100,000 µs)
    min_log = 0.0
    max_log = 5.0
    span_log = max_log - min_log

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
        f'<text x="{plot_x}" y="{0x24}" fill="#4c4f69" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox NVMe Persistence &amp; Journal Latency'
        f'</text>'
    )
    svg.append(
        f'<text x="{plot_x}" y="{0x3A}" fill="#6c6f85" font-size="{0x0A}">'
        f'Logarithmic latency profile (µs) for disk-bound ingest and churn'
        f'</text>'
    )

    # Gridlines
    grid_decades = [0, 1, 2, 3, 4, 5]
    for dec in grid_decades:
        y = plot_y + plot_h - ((dec - min_log) / span_log) * plot_h
        svg.append(
            f'<line x1="{plot_x}" y1="{y:.1f}" x2="{plot_x + plot_w}" '
            f'y2="{y:.1f}" stroke="#dce0e8" stroke-width="1"/>'
        )
        label = f"10^{dec} µs" if dec > 0 else "1 µs"
        svg.append(
            f'<text x="{plot_x - 8}" y="{y + 4:.1f}" fill="#8c8fa1" '
            f'font-size="{0x09}" text-anchor="end">{label}</text>'
        )

    # X axis points
    n_pts = len(PERCENTILES)
    x_coords = []
    for idx, (_, p_name) in enumerate(PERCENTILES):
        x = plot_x + (idx / (n_pts - 1)) * plot_w
        x_coords.append(x)
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="#e6e9ef" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="#6c6f85" '
            f'font-size="{0x09}" text-anchor="middle">{p_name}</text>'
        )

    # Draw series
    legend_x = plot_x + plot_w + 0x14
    legend_y = plot_y + 0x10

    for s_idx, (bench, workload, thr, label, color) in enumerate(SERIES):
        entry = lookup.get((bench, workload, thr))
        if not entry:
            continue
        lat = entry.get("latency_micros", {})
        coords = []
        for idx, (p_key, _) in enumerate(PERCENTILES):
            val = lat.get(p_key, 1.0)
            log_val = math.log10(max(val, 1.0))
            clamped = max(min(log_val, max_log), min_log)
            y = plot_y + plot_h - ((clamped - min_log) / span_log) * plot_h
            coords.append((x_coords[idx], y))

        pts_str = " ".join([f"{cx:.1f},{cy:.1f}" for cx, cy in coords])
        svg.append(
            f'<polyline points="{pts_str}" fill="none" stroke="{color}" '
            f'stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>'
        )
        for cx, cy in coords:
            svg.append(
                f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="3.2" '
                f'fill="{color}" stroke="#eff1f5" stroke-width="1.2"/>'
            )

        # Legend entry
        ly = legend_y + s_idx * 0x18
        svg.append(
            f'<rect x="{legend_x}" y="{ly - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{legend_x + 0x10}" y="{ly}" fill="#4c4f69" '
            f'font-size="{0x09}" font-weight="600">{label}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    json_path = os.path.join(root, "tmp", "turbofox_benchmark_results.json")
    out_path = os.path.join(root, "assets", "nvme_persistence_latency.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)
    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
