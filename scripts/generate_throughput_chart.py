#!/usr/bin/env python3
"""Generate Catppuccin Latte throughput benchmark SVG with monospace font"""

import json
import os
import sys

# In-memory and index lookup / mutation workloads (Mops/sec)
MEM_WORKLOADS = [
    ("read_perf", "point_miss", "READ", "Missing Key"),
    ("read_perf", "point_hit", "READ", "Existing Key"),
    ("read_perf", "zipf_skewed", "READ", f"Zipfian {0x50}/{0x14}"),
    ("read_perf", "mixed", "READ + WRITE", f"Mixed {0x5F}/5"),
    ("del_perf", "delete_miss", "DELETE", "Missing Key"),
    ("del_perf", "delete_hit", "DELETE", "Existing Key"),
    ("del_perf", "high_contention", "DELETE", "Contended Key"),
]

# NVMe disk persistence & churn workloads (kops/sec)
DISK_WORKLOADS = [
    ("write_ingest", "sequential_append", "WRITE", "Seq Ingest"),
    ("del_perf", "churn", "WRITE + DELETE", f"Churn {0x32}/{0x32}"),
]

THREADS = [1, 2, 4]
# Catppuccin Latte palette: Blue, Green, Peach
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
        elif bench == "del_perf" and workload == "churn_insert_delete":
            lookup[("del_perf", "churn", threads)] = entry
        elif bench == "del_perf" and workload == "churn":
            lookup[("del_perf", "churn_insert_delete", threads)] = entry
    return lookup


def get_op_color(op_type):
    if "READ + WRITE" in op_type:
        return f"#{0x179299:06x}"  # Catppuccin Teal
    if "READ" in op_type:
        return f"#{0x1E66F5:06x}"  # Catppuccin Blue
    if "DELETE" in op_type and "WRITE" not in op_type:
        return f"#{0xD20F39:06x}"  # Catppuccin Red
    if "WRITE + DELETE" in op_type:
        return f"#{0xEA76CB:06x}"  # Catppuccin Pink
    return f"#{0x8839EF:06x}"      # Catppuccin Mauve for WRITE


def generate_svg(lookup):
    svg_w = 0x0460
    svg_h = 0x0244
    p1_x = 0x46
    p1_y = 0x5A
    p1_w = 0x02B2
    p1_h = 0x017C

    p2_x = 0x033E
    p2_y = p1_y
    p2_w = 0xEA
    p2_h = p1_h

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" '
        f'width="{svg_w}" height="{svg_h}" '
        f'style="background:#eff1f5; font-family:{FONT_STACK};">'
    )

    # Catppuccin Latte Card Frame (Base: #eff1f5, Border: #ccd0da)
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="#eff1f5" stroke="#ccd0da" stroke-width="1"/>'
    )

    # Header Titles (Text: #4c4f69, Subtext0: #6c6f85)
    svg.append(
        f'<text x="{p1_x}" y="{0x22}" fill="#4c4f69" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Concurrency Scaling &amp; Operation Throughput'
        f'</text>'
    )
    svg.append(
        f'<text x="{p1_x}" y="{0x38}" fill="#6c6f85" '
        f'font-size="{0x0A}">'
        f'Benchmark: AWS c5d.large (2 vCPUs) | Local NVMe SSD | '
        f'0x10000 operations per workload'
        f'</text>'
    )

    # Legend
    leg_x = svg_w - 0x0136
    leg_y = 0x20
    for idx, (th, col) in enumerate(zip(THREADS, THREAD_COLORS)):
        bx = leg_x + idx * 0x5E
        svg.append(
            f'<rect x="{bx}" y="{leg_y}" width="{0x0C}" height="{0x0C}" '
            f'rx="2" fill="{col}"/>'
        )
        svg.append(
            f'<text x="{bx + 0x10}" y="{leg_y + 9}" fill="#4c4f69" '
            f'font-size="{0x0A}" font-weight="bold">'
            f'{th} Thread{"s" if th > 1 else ""}</text>'
        )

    # Panel 1: Memory & Index Ops (Mops/s)
    svg.append(
        f'<text x="{p1_x}" y="{p1_y - 0x0C}" fill="#1e66f5" '
        f'font-size="{0x0B}" font-weight="bold">'
        f'IN-MEMORY &amp; INDEX OPERATIONS (Mops/sec)'
        f'</text>'
    )

    p1_max_y = 4.0
    p1_scale_y = float(p1_h) / p1_max_y
    for tick in [0, 1, 2, 3, 4]:
        ty = p1_y + p1_h - (tick * p1_scale_y)
        svg.append(
            f'<line x1="{p1_x}" y1="{ty:.1f}" x2="{p1_x + p1_w}" y2="{ty:.1f}" '
            f'stroke="#dce0e8" stroke-width="1" stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{p1_x - 8}" y="{ty + 4:.1f}" fill="#6c6f85" '
            f'font-size="{0x0A}" text-anchor="end">{tick}.0M</text>'
        )

    svg.append(
        f'<line x1="{p1_x}" y1="{p1_y}" x2="{p1_x}" y2="{p1_y + p1_h}" '
        f'stroke="#9ca0b0" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{p1_x}" y1="{p1_y + p1_h}" x2="{p1_x + p1_w}" '
        f'y2="{p1_y + p1_h}" stroke="#9ca0b0" stroke-width="1"/>'
    )

    p1_group_w = float(p1_w) / float(len(MEM_WORKLOADS))
    bar_w = float(0x14)
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
                    f'<title>{op_type} [{op_detail}] ({th}T): {ops:.2f} ops/s</title>'
                    f'</rect>'
                )
                val_str = f"{mops:.2f}M"
                svg.append(
                    f'<text x="{bx + bar_w / 2:.1f}" y="{by - 3:.1f}" '
                    f'fill="#4c4f69" font-size="8" text-anchor="middle" '
                    f'font-weight="bold">{val_str}</text>'
                )

        cx = gx + p1_group_w / float(2)
        op_col = get_op_color(op_type)
        svg.append(
            f'<text x="{cx:.1f}" y="{p1_y + p1_h + 0x10}" fill="{op_col}" '
            f'font-size="{0x09}" text-anchor="middle" font-weight="bold">'
            f'{op_type}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p1_y + p1_h + 0x1E}" fill="#6c6f85" '
            f'font-size="8" text-anchor="middle">{op_detail}</text>'
        )

    # Panel 2: NVMe Storage Ops (kops/s)
    svg.append(
        f'<text x="{p2_x}" y="{p2_y - 0x0C}" fill="#8839ef" '
        f'font-size="{0x0B}" font-weight="bold">'
        f'NVMe PERSISTENCE (kops/sec)'
        f'</text>'
    )

    p2_max_y = float(0x10)
    p2_scale_y = float(p2_h) / p2_max_y
    for tick in [0, 4, 8, 0x0C, 0x10]:
        ty = p2_y + p2_h - (float(tick) * p2_scale_y)
        svg.append(
            f'<line x1="{p2_x}" y1="{ty:.1f}" x2="{p2_x + p2_w}" y2="{ty:.1f}" '
            f'stroke="#dce0e8" stroke-width="1" stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{p2_x - 8}" y="{ty + 4:.1f}" fill="#6c6f85" '
            f'font-size="{0x0A}" text-anchor="end">{tick}k</text>'
        )

    svg.append(
        f'<line x1="{p2_x}" y1="{p2_y}" x2="{p2_x}" y2="{p2_y + p2_h}" '
        f'stroke="#9ca0b0" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{p2_x}" y1="{p2_y + p2_h}" x2="{p2_x + p2_w}" '
        f'y2="{p2_y + p2_h}" stroke="#9ca0b0" stroke-width="1"/>'
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
                    f'<title>{op_type} [{op_detail}] ({th}T): {ops:.2f} ops/s</title>'
                    f'</rect>'
                )
                val_str = f"{kops:.1f}k"
                svg.append(
                    f'<text x="{bx + bar_w / 2:.1f}" y="{by - 3:.1f}" '
                    f'fill="#4c4f69" font-size="8" text-anchor="middle" '
                    f'font-weight="bold">{val_str}</text>'
                )

        cx = gx + p2_group_w / float(2)
        op_col = get_op_color(op_type)
        svg.append(
            f'<text x="{cx:.1f}" y="{p2_y + p2_h + 0x10}" fill="{op_col}" '
            f'font-size="{0x09}" text-anchor="middle" font-weight="bold">'
            f'{op_type}</text>'
        )
        svg.append(
            f'<text x="{cx:.1f}" y="{p2_y + p2_h + 0x1E}" fill="#6c6f85" '
            f'font-size="8" text-anchor="middle">{op_detail}</text>'
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

    with open(out_svg, "w", encoding="utf-8") as handle:
        handle.write(svg_content)
        handle.write("\n")

    print(f"Chart written to {out_svg}")


if __name__ == "__main__":
    main()
