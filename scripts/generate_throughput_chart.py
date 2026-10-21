#!/usr/bin/env python3
"""Generate Rosé Pine Dawn throughput benchmark SVG with monospace font"""

import json
import os
import sys
import xml.etree.ElementTree as ET

# Rosé Pine Dawn palette tokens
COLOR_BASE = "#faf4ed"
COLOR_SURFACE = "#fffaf3"
COLOR_OVERLAY = "#f2e9e1"
COLOR_MUTED = "#9893a5"
COLOR_SUBTLE = "#797593"
COLOR_TEXT = "#575279"
COLOR_BORDER = "#dfdad9"
COLOR_LINE = "#cecacd"

# Accent series tokens
ACCENT_PINE = "#286983"
ACCENT_IRIS = "#907aa9"
ACCENT_LOVE = "#b4637a"
ACCENT_GOLD = "#ea9d34"
ACCENT_FOAM = "#56949f"
ACCENT_ROSE = "#d7827e"

# In-memory and index lookup / mutation workloads (Mops/sec)
MEM_WORKLOADS = [
    ("read_perf", "point_miss", "READ", "Point Miss (Absent)"),
    ("read_perf", "point_hit", "READ", "Point Hit (Existing)"),
    ("read_perf", "zipf_skewed", "READ", f"Zipfian {0x50}/{0x14} Skew"),
    ("read_perf", "mixed", "READ/WRITE", f"Mixed {0x5F}/5 OLTP"),
    ("del_perf", "delete_miss", "DELETE", "Point Miss (Absent)"),
    ("del_perf", "delete_hit", "DELETE", "Point Hit (Existing)"),
    ("del_perf", "high_contention", "DELETE", "Hot-Key Contention"),
]

# NVMe disk persistence & churn workloads (kops/sec)
DISK_WORKLOADS = [
    ("write_ingest", "sequential_append", "WAL", "Sequential Ingest"),
    ("del_perf", "churn", "CHURN", f"Churn {0x32}/{0x32}"),
]

THREADS = [1, 2, 4]
THREAD_COLORS = [ACCENT_PINE, ACCENT_IRIS, ACCENT_LOVE]

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
    return lookup


def get_op_color(op_type):
    if "READ/WRITE" in op_type:
        return ACCENT_GOLD
    if "READ" in op_type:
        return ACCENT_PINE
    if "DELETE" in op_type:
        return ACCENT_LOVE
    if "CHURN" in op_type:
        return ACCENT_ROSE
    if "WAL" in op_type:
        return ACCENT_IRIS
    return ACCENT_PINE


def generate_svg(lookup):
    svg_w = 0x04C4
    svg_h = 0x0218
    p1_x = 0x46
    p1_y = 0x54
    p1_w = 0x02F0
    p1_h = 0x0150

    p2_x = 0x0380
    p2_y = p1_y
    p2_w = 0x0100
    p2_h = p1_h

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" '
        f'width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )

    # Rosé Pine Dawn Card Frame
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Header Titles
    title_x = 0x24
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Concurrency Scaling &amp; Operation Throughput'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" '
        f'font-size="{0x0A}">'
        f'Benchmark: AWS c5d.large (2 vCPUs) | Local NVMe SSD | '
        f'0x10000 operations per workload'
        f'</text>'
    )

    # Legend
    leg_x = svg_w - 0x0118
    leg_y = 0x22
    for idx, (th, col) in enumerate(zip(THREADS, THREAD_COLORS)):
        bx = leg_x + idx * 0x36
        svg.append(
            f'<rect x="{bx}" y="{leg_y - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{col}"/>'
        )
        svg.append(
            f'<text x="{bx + 0x0E}" y="{leg_y}" fill="{COLOR_TEXT}" '
            f'font-size="{0x09}" font-weight="bold">'
            f'{th}T</text>'
        )

    # Panel 1: Memory & Index Ops (Mops/s)
    svg.append(
        f'<text x="{p1_x}" y="{p1_y - 0x0A}" fill="{COLOR_TEXT}" '
        f'font-size="{0x0A}" font-weight="bold">'
        f'IN-MEMORY &amp; INDEX OPERATIONS (Mops/sec)'
        f'</text>'
    )

    p1_max_y = 4.0
    p1_scale_y = float(p1_h) / p1_max_y
    for tick in [0, 1, 2, 3, 4]:
        ty = p1_y + p1_h - (tick * p1_scale_y)
        svg.append(
            f'<line x1="{p1_x}" y1="{ty:.1f}" x2="{p1_x + p1_w}" y2="{ty:.1f}" '
            f'stroke="{COLOR_OVERLAY}" stroke-width="1" stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{p1_x - 8}" y="{ty + 3:.1f}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="end">{tick}.0M</text>'
        )

    svg.append(
        f'<line x1="{p1_x}" y1="{p1_y}" x2="{p1_x}" y2="{p1_y + p1_h}" '
        f'stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{p1_x}" y1="{p1_y + p1_h}" x2="{p1_x + p1_w}" '
        f'y2="{p1_y + p1_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    p1_group_w = float(p1_w) / float(len(MEM_WORKLOADS))
    bar_w = float(0x12)
    bar_gap = float(2)
    p1_pad = (p1_group_w - (float(3) * bar_w + float(2) * bar_gap)) / float(2)

    for g_idx, (bench, pat, op_type, op_detail) in enumerate(MEM_WORKLOADS):
        gx = p1_x + g_idx * p1_group_w

        for t_idx, (th, color) in enumerate(zip(THREADS, THREAD_COLORS)):
            entry = next(
                (e for (b, w, t), e in lookup.items()
                 if b == bench and pat in w and t == th), None
            )
            if entry:
                ops = entry["throughput_ops_sec"]
                mops = ops / float(0x0F4240)
                bh = min(mops * p1_scale_y, float(p1_h))
                bx = gx + p1_pad + t_idx * (bar_w + bar_gap)
                by = p1_y + p1_h - bh

                svg.append(
                    f'<rect x="{bx:.1f}" y="{by:.1f}" width="{bar_w:.1f}" '
                    f'height="{bh:.1f}" rx="2" fill="{color}">'
                    f'<title>{op_type}: {op_detail} ({th}T): {ops:.2f} ops/s</title>'
                    f'</rect>'
                )
                val_str = f"{mops:.2f}M"
                svg.append(
                    f'<text x="{bx + bar_w / 2:.1f}" y="{by - 3:.1f}" '
                    f'fill="{color}" font-size="{0x07}" text-anchor="middle" '
                    f'font-weight="bold">{val_str}</text>'
                )

        cx = gx + p1_group_w / float(2)
        op_col = get_op_color(op_type)
        svg.append(
            f'<text x="{cx:.1f}" y="{p1_y + p1_h + 0x10}" fill="{op_col}" '
            f'font-size="{0x08}" text-anchor="middle" font-weight="bold">'
            f'{op_type}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p1_y + p1_h + 0x1C}" fill="{COLOR_SUBTLE}" '
            f'font-size="{0x07}" text-anchor="middle">{op_detail}</text>'
        )

    # Panel 2: NVMe Storage Ops (kops/s)
    svg.append(
        f'<text x="{p2_x}" y="{p2_y - 0x0A}" fill="{COLOR_TEXT}" '
        f'font-size="{0x0A}" font-weight="bold">'
        f'NVMe PERSISTENCE (kops/sec)'
        f'</text>'
    )

    p2_max_y = float(0x10)
    p2_scale_y = float(p2_h) / p2_max_y
    for tick in [0, 4, 8, 0x0C, 0x10]:
        ty = p2_y + p2_h - (float(tick) * p2_scale_y)
        svg.append(
            f'<line x1="{p2_x}" y1="{ty:.1f}" x2="{p2_x + p2_w}" y2="{ty:.1f}" '
            f'stroke="{COLOR_OVERLAY}" stroke-width="1" stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{p2_x - 8}" y="{ty + 3:.1f}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="end">{tick}k</text>'
        )

    svg.append(
        f'<line x1="{p2_x}" y1="{p2_y}" x2="{p2_x}" y2="{p2_y + p2_h}" '
        f'stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{p2_x}" y1="{p2_y + p2_h}" x2="{p2_x + p2_w}" '
        f'y2="{p2_y + p2_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    p2_group_w = float(p2_w) / float(len(DISK_WORKLOADS))
    p2_pad = (p2_group_w - (float(3) * bar_w + float(2) * bar_gap)) / float(2)

    for g_idx, (bench, pat, op_type, op_detail) in enumerate(DISK_WORKLOADS):
        gx = p2_x + g_idx * p2_group_w

        for t_idx, (th, color) in enumerate(zip(THREADS, THREAD_COLORS)):
            entry = next(
                (e for (b, w, t), e in lookup.items()
                 if b == bench and pat in w and t == th), None
            )
            if entry:
                ops = entry["throughput_ops_sec"]
                kops = ops / float(0x03E8)
                bh = min(kops * p2_scale_y, float(p2_h))
                bx = gx + p2_pad + t_idx * (bar_w + bar_gap)
                by = p2_y + p2_h - bh

                svg.append(
                    f'<rect x="{bx:.1f}" y="{by:.1f}" width="{bar_w:.1f}" '
                    f'height="{bh:.1f}" rx="2" fill="{color}">'
                    f'<title>{op_type}: {op_detail} ({th}T): {ops:.2f} ops/s</title>'
                    f'</rect>'
                )
                val_str = f"{kops:.1f}k"
                svg.append(
                    f'<text x="{bx + bar_w / 2:.1f}" y="{by - 3:.1f}" '
                    f'fill="{color}" font-size="{0x07}" text-anchor="middle" '
                    f'font-weight="bold">{val_str}</text>'
                )

        cx = gx + p2_group_w / float(2)
        op_col = get_op_color(op_type)
        svg.append(
            f'<text x="{cx:.1f}" y="{p2_y + p2_h + 0x10}" fill="{op_col}" '
            f'font-size="{0x08}" text-anchor="middle" font-weight="bold">'
            f'{op_type}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p2_y + p2_h + 0x1C}" fill="{COLOR_SUBTLE}" '
            f'font-size="{0x07}" text-anchor="middle">{op_detail}</text>'
        )

    svg.append('</svg>')
    return "\n".join(svg)


def main():
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    src_json = os.path.join(repo_root, "tmp", "turbofox_benchmark_results.json")
    out_dir = os.path.join(repo_root, "assets")
    out_svg = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        out_dir, "throughput_scaling.svg"
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
