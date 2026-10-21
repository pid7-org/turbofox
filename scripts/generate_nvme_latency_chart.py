#!/usr/bin/env python3
"""Generate Catppuccin Latte NVMe persistence latency distribution SVG with monospace font"""

import json
import math
import os
import sys
import xml.etree.ElementTree as ET

COLOR_BASE = "#eff1f5"
COLOR_SURFACE = "#e6e9ef"
COLOR_OVERLAY = "#ccd0da"
COLOR_MUTED = "#8c8fa1"
COLOR_SUBTLE = "#6c6f85"
COLOR_TEXT = "#4c4f69"
COLOR_BORDER = "#ccd0da"
COLOR_LINE = "#bcc0cc"

ACCENT_BLUE = "#1e66f5"
ACCENT_MAUVE = "#8839ef"
ACCENT_PEACH = "#fe640b"
ACCENT_RED = "#d20f39"

SERIES = [
    ("write_ingest", "sequential_append", 1, "WAL: Ingest (1T)", ACCENT_BLUE),
    ("write_ingest", "sequential_append", 4, "WAL: Ingest (4T)", ACCENT_MAUVE),
    ("del_perf", "churn_insert_delete", 1, f"CHURN: {0x32}/{0x32} (1T)", ACCENT_PEACH),
    ("del_perf", "churn_insert_delete", 4, f"CHURN: {0x32}/{0x32} (4T)", ACCENT_RED),
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
    plot_x = 0x58
    plot_y = 0x54
    plot_w = 0x0310
    plot_h = 0x0140

    min_log = 0.0
    max_log = 5.0
    span_log = max_log - min_log

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
        f'TurboFox NVMe Persistence &amp; Journal Latency Spectrum'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" font-size="{0x0A}">'
        f'Logarithmic latency profile (µs) across worker threads (Catppuccin Latte theme)'
        f'</text>'
    )

    grid_decades = [0, 1, 2, 3, 4, 5]
    for dec in grid_decades:
        y = plot_y + plot_h - ((dec - min_log) / span_log) * plot_h
        svg.append(
            f'<line x1="{plot_x}" y1="{y:.1f}" x2="{plot_x + plot_w}" '
            f'y2="{y:.1f}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        label = f"{0x0A}^{dec} µs" if dec > 0 else "1 µs"
        svg.append(
            f'<text x="{plot_x - 8}" y="{y + 3:.1f}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="end">{label}</text>'
        )

    n_pts = len(PERCENTILES)
    x_coords = []
    for idx, (_, p_name) in enumerate(PERCENTILES):
        x = plot_x + (idx / (n_pts - 1)) * plot_w
        x_coords.append(x)
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{p_name}</text>'
        )

    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y + plot_h}" x2="{plot_x + plot_w}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    legend_x = plot_x + plot_w + 0x18
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
                f'fill="{color}" stroke="{COLOR_BASE}" stroke-width="1.2"/>'
            )

        ly = legend_y + s_idx * 0x18
        svg.append(
            f'<rect x="{legend_x}" y="{ly - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{legend_x + 0x10}" y="{ly}" fill="{COLOR_TEXT}" '
            f'font-size="{0x08}" font-weight="bold">{label}</text>'
        )

    note_x = title_x
    note_y = plot_y + plot_h + 0x2C
    note_lines = [
        "* Note: Logarithmic scale spans 1 µs to 0x000186A0 µs across 1T and 4T worker threads; "
        "WAL sequential append logging evaluated with explicit sync barriers; "
        f"steady-state churn ({0x32}/{0x32}) measures sustained slot reuse and physical "
        "file stability",
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
    out_path = os.path.join(root, "assets", "nvme_persistence_latency.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)

    ET.fromstring(svg)

    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
        handle.write("\n")
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
