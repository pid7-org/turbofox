#!/usr/bin/env bash
#
# TurboFox AWS EC2 Instance Provisioner & Benchmark Initializer
#
# Execution:
#   chmod +x scripts/init_ec2_bench.sh
#   ./scripts/init_ec2_bench.sh
#
# Auto-run benchmarks after setup:
#   ./scripts/init_ec2_bench.sh --run --ops 0x186A0 --out results.json

set -euo pipefail

AUTO_RUN=0
REPO_URL="https://github.com/pid7-org/turbofox.git"
BENCH_DIR="/mnt/nvme0n1"
PASSTHROUGH_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run)
      AUTO_RUN=1
      shift 1
      ;;
    --repo)
      REPO_URL="$2"
      shift 2
      ;;
    --bench-dir)
      BENCH_DIR="$2"
      shift 2
      ;;
    *)
      PASSTHROUGH_ARGS+=("$1")
      shift 1
      ;;
  esac
done

SUDO=""
if [[ $(id -u) -ne 0 ]]; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  else
    echo "Error: elevated privileges required; install sudo or run as root" >&2
    exit 1
  fi
fi

echo "=== [1/5] Provisioning OS dependencies ===" >&2
if command -v apt-get >/dev/null 2>&1; then
  $SUDO apt-get update -y
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y \
    build-essential git curl xfsprogs util-linux python3 linux-tools-common
elif command -v dnf >/dev/null 2>&1; then
  $SUDO dnf update -y
  $SUDO dnf install -y gcc make git curl xfsprogs util-linux python3 kernel-tools
fi

echo "=== [2/5] Tuning CPU scaling governor to performance ===" >&2
if command -v cpupower >/dev/null 2>&1; then
  $SUDO cpupower frequency-set -g performance >/dev/null 2>&1 || true
fi

for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  if [[ -f "${gov}" ]]; then
    echo "performance" | $SUDO tee "${gov}" >/dev/null 2>&1 || true
  fi
done

echo "=== [3/5] Initializing local NVMe instance storage ===" >&2
if ! mountpoint -q "${BENCH_DIR}"; then
  NVME_DEV=""
  for dev in /dev/nvme*n1; do
    if [[ -b "${dev}" ]] && ! lsblk -no MOUNTPOINTS "${dev}" | grep -q "/"; then
      NVME_DEV="${dev}"
      break
    fi
  done

  if [[ -n "${NVME_DEV}" ]]; then
    echo "Formatting ${NVME_DEV} with XFS..." >&2
    $SUDO mkfs.xfs -f "${NVME_DEV}"
    $SUDO mkdir -p "${BENCH_DIR}"
    $SUDO mount -o noatime,nodiratime,logbufs=8 "${NVME_DEV}" "${BENCH_DIR}"
    $SUDO chmod 0x01FF "${BENCH_DIR}"
  else
    echo "No unmounted NVMe block device found; falling back to /tmp/turbofox_bench" >&2
    BENCH_DIR="/tmp/turbofox_bench"
    mkdir -p "${BENCH_DIR}"
  fi
fi

echo "=== [4/5] Configuring Rust toolchain ===" >&2
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal
fi

# shellcheck source=/dev/null
source "${HOME}/.cargo/env"
rustup default stable
rustup update stable

echo "=== [5/5] Synchronizing repository and runner ===" >&2
WORK_DIR="${PWD}"
HAS_CARGO=0
if [[ -f "${WORK_DIR}/Cargo.toml" ]] && grep -q 'name = "turbofox"' "${WORK_DIR}/Cargo.toml"; then
  HAS_CARGO=1
fi

if [[ ${HAS_CARGO} -eq 0 ]]; then
  TARGET_CLONE="${HOME}/turbofox"
  if [[ ! -d "${TARGET_CLONE}" ]]; then
    git clone "${REPO_URL}" "${TARGET_CLONE}"
  fi
  WORK_DIR="${TARGET_CLONE}"
fi

RUNNER="${WORK_DIR}/scripts/run_ec2_benches.sh"
chmod +x "${RUNNER}"

echo "Host setup completed successfully" >&2
echo "Repository: ${WORK_DIR}" >&2
echo "Benchmark directory: ${BENCH_DIR}" >&2

if [[ ${AUTO_RUN} -eq 1 ]]; then
  echo "Launching benchmark runner..." >&2
  cd "${WORK_DIR}"
  "${RUNNER}" --dir "${BENCH_DIR}" "${PASSTHROUGH_ARGS[@]}"
fi
