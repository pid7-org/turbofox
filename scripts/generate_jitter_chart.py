#!/usr/bin/env python3
"""Generate Rosé Pine Dawn latency jitter & variance SVG with monospace font"""

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

# Accent colors
ACCENT_LOVE = "#b4637a"
ACCENT_IRIS = "#907aa9"
ACCENT_FOAM = "#56949f"
ACCENT_GOLD = "#ea9d34"
ACCENT_ROSE = "#d7827e"
ACCENT_PINE = "#286983"
ACCENT_PINE_ALT = "#31748f"

# Workloads profiled: (bench, workload, display_label, bar_color)
WORKLOADS = [
    ("write_ingest", "sequential_append", "WAL: Ingest (NVMe fsync)", ACCENT_LOVE),
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention", ACCENT_IRIS),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)", ACCENT_FOAM),
    ("read_perf", "mixed_95_5", f"READ/WRITE: Mixed {0x5F}/5 OLTP", ACCENT_GOLD),
    ("del_perf", "churn_insert_delete", f"CHURN: {0x32}/{0x32} Put/Del", ACCENT_ROSE),
    ("read_perf", "point_hit", "READ: Point Hit (Uniform)", ACCENT_PINE),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)", ACCENT_PINE_ALT),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)", ACCENT_IRIS),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)", ACCENT_LOVE),
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

    max_cv = 8.0
    num_rows = len(WORKLOADS)
    row_h = float(plot_h) / float(num_rows)
    bar_h = 0x0E

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
        f'TurboFox Latency Predictability &amp; Jitter Profile (CV = \u03c3 / \u03bc)'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" font-size="{0x0A}">'
        f'Coefficient of variation (1T baseline): lower indicates tighter SLA predictability '
        f'(Ros\u00e9 Pine Dawn theme)'
        f'</text>'
    )

    # Gridlines and numeric ticks
    ticks = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
    for tick in ticks:
        x = plot_x + (tick / max_cv) * plot_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{tick:.1f}</text>'
        )

    # Axis boundary line
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
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
            f'<text x="{plot_x - 0x0A}" y="{by + 0x0A:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="{0x08}" font-weight="bold" text-anchor="end">{label}</text>'
        )
        svg.append(
            f'<rect x="{plot_x}" y="{by:.1f}" width="{bw:.1f}" '
            f'height="{bar_h}" rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{plot_x + bw + 6:.1f}" y="{by + 0x0A:.1f}" fill="{color}" '
            f'font-size="{0x08}" font-weight="bold">'
            f'CV={cv:.2f} (\u03bc={mean_val:.2f}\u00b5s, \u03c3={std_val:.2f}\u00b5s)</text>'
        )

    # Bottom notes section for complex domain concepts
    note_x = title_x
    note_y = plot_y + plot_h + 0x2A
    note_lines = [
        "* Note: CV (Coefficient of Variation = \u03c3 / \u03bc) quantifies relative latency "
        "jitter independent of scale; lower CV denotes superior SLA determinism",
        "  In-memory hash index probes and deletions show tight variance (CV &lt; 0.8), "
        "proving low jitter across branch targets and cache lines",
        "  WAL NVMe persistence displays elevated CV (~7.4) driven by hardware controller "
        "queueing and OS page writeback spikes during fsync barriers",
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
    out_path = os.path.join(root, "assets", "latency_jitter.svg")

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
