#!/usr/bin/env python3
"""Generate Rosé Pine Dawn latency percentile profile SVG with monospace font"""

import json
import math
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

# Accent colors for series
ACCENT_PINE = "#286983"
ACCENT_FOAM = "#56949f"
ACCENT_IRIS = "#907aa9"
ACCENT_LOVE = "#b4637a"
ACCENT_GOLD = "#ea9d34"
ACCENT_ROSE = "#d7827e"
ACCENT_PINE_ALT = "#31748f"

# Core operations profiled at 1-thread baseline (isolated core performance)
SERIES_SPECS = [
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention", ACCENT_IRIS),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)", ACCENT_FOAM),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)", ACCENT_ROSE),
    ("read_perf", "point_hit", "READ: Point Hit (Uniform)", ACCENT_PINE),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)", ACCENT_GOLD),
    ("read_perf", "mixed", f"READ/WRITE: Mixed {0x5F}/5 OLTP", ACCENT_PINE_ALT),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)", ACCENT_LOVE),
]

PERCENTILES = [
    ("p50", "p50"),
    ("p75", "p75"),
    ("p90", "p90"),
    ("p95", "p95"),
    ("p99", "p99"),
    ("p999", "p99.9"),
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
        if bench == "read_perf" and workload == "mixed_hit_miss":
            lookup[("read_perf", "mixed", threads)] = entry
        elif bench == "read_perf" and workload == "mixed_95_5":
            lookup[("read_perf", "mixed", threads)] = entry
        elif bench == "read_perf" and workload == "mixed":
            lookup[("read_perf", "mixed_hit_miss", threads)] = entry
        elif bench == "del_perf" and workload == "mixed_delete":
            lookup[("del_perf", "mixed", threads)] = entry
        elif bench == "del_perf" and workload == "mixed":
            lookup[("del_perf", "mixed_delete", threads)] = entry
        elif bench == "del_perf" and workload == "churn_insert_delete":
            lookup[("del_perf", "churn", threads)] = entry
        elif bench == "del_perf" and workload == "churn":
            lookup[("del_perf", "churn_insert_delete", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x20":
            lookup[("sync_perf", "0x20", threads)] = entry
        elif bench == "sync_perf" and workload == "0x20":
            lookup[("sync_perf", "pages_0x20", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x0100":
            lookup[("sync_perf", "0x0100", threads)] = entry
        elif bench == "sync_perf" and workload == "0x0100":
            lookup[("sync_perf", "pages_0x0100", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x0400":
            lookup[("sync_perf", "0x0400", threads)] = entry
        elif bench == "sync_perf" and workload == "0x0400":
            lookup[("sync_perf", "pages_0x0400", threads)] = entry
        elif bench == "churn_perf" and workload == "steady_state_churn":
            lookup[("churn_perf", "steady_state", threads)] = entry
        elif bench == "churn_perf" and workload == "steady_state":
            lookup[("churn_perf", "steady_state_churn", threads)] = entry
        elif bench == "write_perf" and workload == "pure_append":
            lookup[("write_ingest", "sequential_append", threads)] = entry
    return lookup


def generate_svg(lookup):
    svg_w = 0x0400
    svg_h = 0x021C
    plot_x = 0x50
    plot_y = 0x54
    plot_w = 0x0244
    plot_h = 0x0168

    min_log = -1.0
    max_log = 2.0
    span_log = max_log - min_log

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" '
        f'width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )

    # Rosé Pine Dawn Frame
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Title & Subtitle
    title_x = 0x24
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox In-Memory Latency Percentile Profile'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" '
        f'font-size="{0x0A}">'
        f'Platform: AWS c5d.large (1 Thread baseline) | '
        f'Logarithmic latency scale (0.1 µs to {0x64} µs)'
        f'</text>'
    )

    # Sub-microsecond boundary indicator (1.0 µs line)
    y_1us = plot_y + plot_h - ((0.0 - min_log) / span_log * float(plot_h))
    svg.append(
        f'<rect x="{plot_x}" y="{y_1us:.1f}" width="{plot_w}" '
        f'height="{plot_y + plot_h - y_1us:.1f}" fill="{ACCENT_FOAM}" '
        f'fill-opacity="0.08"/>'
    )
    # Position SUB-MICROSECOND REGION text near the bottom border
    text_y = plot_y + plot_h - 0x08
    svg.append(
        f'<text x="{plot_x + plot_w - 0x08}" y="{text_y:.1f}" '
        f'fill="{ACCENT_FOAM}" font-size="{0x08}" font-weight="bold" '
        f'text-anchor="end">'
        f'SUB-MICROSECOND REGION (&lt; 1.0 µs)'
        f'</text>'
    )

    # Y-axis major gridlines
    y_ticks = [
        (0.1, "0.1 µs"),
        (1.0, "1.0 µs"),
        (float(0x0A), f"{0x0A} µs"),
        (float(0x64), f"{0x64} µs"),
    ]

    for val, label in y_ticks:
        norm_y = (math.log10(val) - min_log) / span_log
        ty = plot_y + plot_h - (norm_y * float(plot_h))
        svg.append(
            f'<line x1="{plot_x}" y1="{ty:.1f}" x2="{plot_x + plot_w}" '
            f'y2="{ty:.1f}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{plot_x - 8}" y="{ty + 3:.1f}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="end">{label}</text>'
        )

    # Y-axis minor gridlines
    minor_vals = [
        0.2, 0.5, 2.0, 5.0,
        float(0x14), float(0x32)
    ]
    for val in minor_vals:
        norm_y = (math.log10(val) - min_log) / span_log
        ty = plot_y + plot_h - (norm_y * float(plot_h))
        svg.append(
            f'<line x1="{plot_x}" y1="{ty:.1f}" x2="{plot_x + plot_w}" '
            f'y2="{ty:.1f}" stroke="{COLOR_OVERLAY}" stroke-width="1" '
            f'stroke-dasharray="2,2"/>'
        )

    # X-axis coordinate calculator
    num_pct = len(PERCENTILES)
    x_step = float(plot_w) / float(num_pct - 1)
    pct_x_coords = [plot_x + i * x_step for i in range(num_pct)]

    for x_c, (_, label) in zip(pct_x_coords, PERCENTILES):
        svg.append(
            f'<line x1="{x_c:.1f}" y1="{plot_y}" x2="{x_c:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1" '
            f'stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{x_c:.1f}" y="{plot_y + plot_h + 0x12}" '
            f'fill="{COLOR_TEXT}" font-size="{0x08}" font-weight="bold" '
            f'text-anchor="middle">{label}</text>'
        )

    # Axes boundary lines
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y + plot_h}" '
        f'x2="{plot_x + plot_w}" y2="{plot_y + plot_h}" '
        f'stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    # Series lines and markers
    for bench, pat, label, color in SERIES_SPECS:
        entry = next(
            (e for (b, w, t), e in lookup.items()
             if b == bench and pat in w and t == 1), None
        )
        if not entry:
            continue

        lat_table = entry.get("latency_micros", {})
        coords = []
        for x_c, (pkey, plabel) in zip(pct_x_coords, PERCENTILES):
            val = lat_table.get(pkey, 0.1)
            clamped = max(val, 0.1)
            norm_y = (math.log10(clamped) - min_log) / span_log
            y_c = plot_y + plot_h - (norm_y * float(plot_h))
            coords.append((x_c, y_c, val, plabel))

        # Polyline path
        pts_str = " ".join(f"{xc:.1f},{yc:.1f}" for xc, yc, _, _ in coords)
        svg.append(
            f'<polyline points="{pts_str}" fill="none" stroke="{color}" '
            f'stroke-width="2"/>'
        )

        # Point markers with tooltips
        for xc, yc, val, plabel in coords:
            svg.append(
                f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="3" fill="{color}">'
                f'<title>{label} ({plabel}): {val:.2f} µs</title>'
                f'</circle>'
            )

    # Legend Panel on the right side
    leg_x = plot_x + plot_w + 0x18
    leg_y = plot_y + 4
    svg.append(
        f'<text x="{leg_x}" y="{leg_y}" fill="{COLOR_TEXT}" '
        f'font-size="{0x08}" font-weight="bold">'
        f'WORKLOAD (1 THREAD)'
        f'</text>'
    )

    for idx, (_, _, label, color) in enumerate(SERIES_SPECS):
        ly = leg_y + 0x12 + idx * 0x16
        svg.append(
            f'<line x1="{leg_x}" y1="{ly}" x2="{leg_x + 0x0E}" y2="{ly}" '
            f'stroke="{color}" stroke-width="2"/>'
        )
        svg.append(
            f'<circle cx="{leg_x + 7}" cy="{ly}" r="3" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{leg_x + 0x14}" y="{ly + 3}" fill="{COLOR_TEXT}" '
            f'font-size="{0x08}" font-weight="bold">{label}</text>'
        )

    svg.append('</svg>')
    return "\n".join(svg)


def main():
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    src_json = os.path.join(repo_root, "tmp", "turbofox_benchmark_results.json")
    out_dir = os.path.join(repo_root, "assets")
    out_svg = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        out_dir, "latency_profile.svg"
    )

    if not os.path.exists(src_json):
        sys.stderr.write(f"benchmark results missing at {src_json}\n")
        sys.exit(1)

    parent_dir = os.path.dirname(out_svg)
    if parent_dir:
        os.makedirs(parent_dir, exist_ok=True)
    lookup = load_results(src_json)
    svg_content = generate_svg(lookup)

    # Validate XML before writing
    ET.fromstring(svg_content)

    with open(out_svg, "w", encoding="utf-8") as handle:
        handle.write(svg_content)
        handle.write("\n")

    print(f"Chart written to {out_svg}")


if __name__ == "__main__":
    main()
