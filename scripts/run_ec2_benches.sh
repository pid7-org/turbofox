#!/usr/bin/env bash
#
# TurboFox AWS EC2 Benchmark Suite Runner
#
# Execution examples:
#   chmod +x scripts/run_ec2_benches.sh
#   ./scripts/run_ec2_benches.sh --ops 0x186A0 --out results.json
#
# Dedicated NVMe instance store execution:
#   ./scripts/run_ec2_benches.sh --ops 0x7A120 --dir /mnt/nvme0n1 --out results.json

set -euo pipefail

DEFAULT_OPS="0x186A0"
DEFAULT_THREADS="1,2,4"
DEFAULT_WARMUP="0x00"
DEFAULT_OUT="turbofox_benchmark_results.json"

OPS="${DEFAULT_OPS}"
THREADS_LIST="${DEFAULT_THREADS}"
WARMUP="${DEFAULT_WARMUP}"
OUT_FILE="${DEFAULT_OUT}"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/turbofox-target}"
BENCH_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ops)
      OPS="$2"
      shift 2
      ;;
    --threads)
      THREADS_LIST="$2"
      shift 2
      ;;
    --dir)
      BENCH_DIR="$2"
      shift 2
      ;;
    --out)
      OUT_FILE="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# Resolve storage directory: prioritize custom, then NVMe mount, fallback /tmp
if [[ -z "${BENCH_DIR}" ]]; then
  if [[ -d "/mnt/nvme0n1" && -w "/mnt/nvme0n1" ]]; then
    BENCH_DIR="/mnt/nvme0n1/turbofox_bench"
  elif [[ -n "${BENCH_DIR:-}" ]]; then
    BENCH_DIR="${BENCH_DIR}"
  else
    BENCH_DIR="/tmp/turbofox_bench"
  fi
fi

mkdir -p "${BENCH_DIR}"

# Collect host metadata
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
KERNEL=$(uname -r)
ARCH=$(uname -m)
CPU_MODEL=$(lscpu | grep "Model name" | sed 's/.*: *//' | tr -d '\n' || echo "Unknown")
TOTAL_CPUS=$(nproc || echo "1")

INSTANCE_TYPE="unknown"
if [[ -f "/sys/devices/virtual/dmi/id/product_name" ]]; then
  INSTANCE_TYPE=$(cat /sys/devices/virtual/dmi/id/product_name | tr -d '\n')
fi

# Build benchmarks in release profile
echo "Compiling benchmark targets in release mode..." >&2
CARGO_TARGET_DIR="${TARGET_DIR}" cargo bench --no-run

DEPS_DIR="${TARGET_DIR}/release/deps"
WRITE_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "write_ingest-*" -executable -type f | head -n 1)
READ_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "read_perf-*" -executable -type f | head -n 1)
DEL_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "del_perf-*" -executable -type f | head -n 1)

if [[ -z "${WRITE_BIN}" || -z "${READ_BIN}" || -z "${DEL_BIN}" ]]; then
  echo "Error: Failed to locate benchmark binaries in ${DEPS_DIR}" >&2
  exit 1
fi

TMP_RESULTS=$(mktemp)
echo "[" > "${TMP_RESULTS}"
FIRST_ENTRY=1

drop_page_cache() {
  sync
  if [[ $(id -u) -eq 0 ]]; then
    echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
  fi
}

run_benchmark() {
  local bin="$1"
  local bench_name="$2"
  local workload="$3"
  local thread_count="$4"

  local tag="${workload:-default}"
  local db_path="${BENCH_DIR}/${bench_name}_${tag}_t${thread_count}_$$"
  rm -rf "${db_path}"*

  drop_page_cache

  local max_core=$((thread_count - 1))
  local taskset_cmd=""
  if command -v taskset >/dev/null 2>&1 && [[ ${thread_count} -le ${TOTAL_CPUS} ]]; then
    taskset_cmd="taskset -c 0-${max_core}"
  fi

  local json_out
  if [[ -n "${workload}" ]]; then
    json_out=$(${taskset_cmd} "${bin}" --ops "${OPS}" --threads "${thread_count}" \
      --workload "${workload}" --warmup "${WARMUP}" --db-path "${db_path}.db")
  else
    json_out=$(${taskset_cmd} "${bin}" --ops "${OPS}" --threads "${thread_count}" \
      --warmup "${WARMUP}" --db-path "${db_path}.db")
  fi

  rm -rf "${db_path}"*

  if [[ ${FIRST_ENTRY} -eq 0 ]]; then
    echo "," >> "${TMP_RESULTS}"
  fi
  echo "${json_out}" >> "${TMP_RESULTS}"
  FIRST_ENTRY=0
}

IFS=',' read -ra THREAD_ARRAY <<< "${THREADS_LIST}"

for T in "${THREAD_ARRAY[@]}"; do
  echo "Running write_ingest (threads=${T})..." >&2
  run_benchmark "${WRITE_BIN}" "write_ingest" "" "${T}"

  for WL in hit miss zipf mixed; do
    echo "Running read_perf [${WL}] (threads=${T})..." >&2
    run_benchmark "${READ_BIN}" "read_perf" "${WL}" "${T}"
  done

  for WL in hit miss churn contention; do
    echo "Running del_perf [${WL}] (threads=${T})..." >&2
    run_benchmark "${DEL_BIN}" "del_perf" "${WL}" "${T}"
  done
done

echo "]" >> "${TMP_RESULTS}"

# Construct consolidated output artifact
cat <<EOF > "${OUT_FILE}"
{
  "metadata": {
    "timestamp_utc": "${TIMESTAMP}",
    "instance_type": "${INSTANCE_TYPE}",
    "cpu_model": "${CPU_MODEL}",
    "kernel": "${KERNEL}",
    "arch": "${ARCH}",
    "storage_path": "${BENCH_DIR}",
    "ops_requested": "${OPS}"
  },
  "results": $(cat "${TMP_RESULTS}")
}
EOF

rm -f "${TMP_RESULTS}"
echo "Benchmarks finished; Results saved to: ${OUT_FILE}" >&2
