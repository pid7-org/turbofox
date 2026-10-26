#!/usr/bin/env bash
#
# TurboFox AWS EC2 Benchmark Suite Runner
# Delegates directly to unified run_benchmarks.sh orchestrator
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${SCRIPT_DIR}/run_benchmarks.sh" "$@"
