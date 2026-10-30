#!/usr/bin/env python3
"""Shim delegating to generate_scale_perf_chart.py"""

import os
import sys

target = os.path.join(os.path.dirname(__file__), "generate_scale_perf_chart.py")
os.execv(sys.executable, [sys.executable, target] + sys.argv[1:])
