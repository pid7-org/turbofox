#!/usr/bin/env bash
#
# TurboFox AWS EC2 Automated Benchmark Lifecycle Runner
#
# Executes Terraform provisioning, validates instance health, runs benchmarks,
# downloads structured JSON telemetry, and validates guaranteed cloud destruction
#
# Execution:
#   chmod +x bencher/run.sh
#   ./bencher/run.sh
#
# Custom benchmark selection:
#   ./bencher/run.sh --benches multi
#
# Custom target region:
#   ./bencher/run.sh --region us-west-2

set -euo pipefail

BENCHER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${BENCHER_DIR}/.." && pwd)"
ASSETS_DIR="${ROOT_DIR}/tmp/assets"
OUTPUT_JSON="${ASSETS_DIR}/turbofox_benchmark_results.json"
SSH_KEY="${BENCHER_DIR}/id_ed25519"
AWS_REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
export AWS_REGION
export AWS_DEFAULT_REGION="${AWS_REGION}"

BRANCH_ARG="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "master")"
BENCHES_ARG="all"
SCALE_OPS_ARG="${SCALE_OPS:-0x8000}"
OPS_ARG="${BENCH_OPS:-0x100000}"

show_help() {
  cat << 'EOF'
Usage: ./bencher/run.sh [OPTIONS] [SCALE_HEX] [KV_HEX]

TurboFox Automated AWS EC2 Benchmark Lifecycle Runner

Provisions a dedicated c6id.xlarge EC2 instance with local NVMe, executes
the selected benchmark suite(s) sequentially, downloads consolidated telemetry,
and guarantees full cloud infrastructure teardown

Arguments:
  [SCALE_HEX]                   Scale benchmark key count in hex (default: 0x8000)
  [KV_HEX]                      KV benchmark ops count in hex (default: 0x100000)

Options:
  --scale-ops, --scale <HEX>    Scale benchmark entries in hex (default: 0x8000)
  --ops, --kv-ops <HEX>         KV benchmarks operation count in hex (default: 0x100000)
  --benches, --bench <TARGETS>  Comma-separated list of benchmarks to run:
                                  all   : scale_perf, kv_perf, kv_perf_multi_tx (default)
                                  scale : scale_perf (single-core interval open/evict)
                                  kv    : kv_perf (single-core 1M ops, 80/20 set, 60/40 get/del)
                                  multi : kv_perf_multi_tx (2-thread concurrent transactions)
  --branch <NAME>               Git branch to clone on remote host (default: active branch)
  --region <AWS_REGION>         AWS deployment region (default: us-east-1)
  -h, --help, help              Show this help message and exit

Examples:
  ./bencher/run.sh                          # Run all 3 benchmarks sequentially (default)
  ./bencher/run.sh 0x8000 0x100000          # Run with custom scale and KV hex counts
  ./bencher/run.sh --scale-ops 0x8000 --ops 0x100000
  ./bencher/run.sh --benches multi          # Run only multi-threaded KV benchmark
  ./bencher/run.sh --region us-west-2       # Run in specific AWS region
EOF
  exit 0
}

normalize_hex() {
  local val="$1"
  local clean="${val//_/}"
  if [[ "${clean}" =~ ^0[xX][0-9a-fA-F]+$ ]]; then
    echo "${clean}"
  elif [[ "${clean}" =~ ^[0-9a-fA-F]+$ ]]; then
    echo "0x${clean}"
  else
    echo "INVALID"
  fi
}

POSITIONAL_HEX=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help|help)
      show_help
      ;;
    --benches|--bench)
      if [[ "${BENCHES_ARG}" == "all" ]]; then
        BENCHES_ARG="$2"
      else
        BENCHES_ARG="${BENCHES_ARG},$2"
      fi
      shift 2
      ;;
    --scale-ops|--scale|--scale-keys)
      SCALE_OPS_ARG="$2"
      shift 2
      ;;
    --ops|--kv-ops|--bench-ops)
      OPS_ARG="$2"
      shift 2
      ;;
    --branch)
      BRANCH_ARG="$2"
      shift 2
      ;;
    --region)
      AWS_REGION="$2"
      export AWS_REGION
      export AWS_DEFAULT_REGION="${AWS_REGION}"
      shift 2
      ;;
    0[xX]*)
      POSITIONAL_HEX+=("$1")
      shift 1
      ;;
    *)
      if [[ "$1" =~ ^[0-9a-fA-F]+$ ]]; then
        POSITIONAL_HEX+=("0x$1")
        shift 1
      else
        echo "Unknown option: $1" >&2
        echo "Run './bencher/run.sh --help' for usage details" >&2
        exit 1
      fi
      ;;
  esac
done

if [[ ${#POSITIONAL_HEX[@]} -ge 1 ]]; then
  SCALE_OPS_ARG="${POSITIONAL_HEX[0]}"
fi
if [[ ${#POSITIONAL_HEX[@]} -ge 2 ]]; then
  OPS_ARG="${POSITIONAL_HEX[1]}"
fi

SCALE_OPS_ARG=$(normalize_hex "${SCALE_OPS_ARG}")
if [[ "${SCALE_OPS_ARG}" == "INVALID" ]]; then
  echo "Error: scale-ops must be a valid hexadecimal value (e.g. 0x8000)" >&2
  exit 1
fi

OPS_ARG=$(normalize_hex "${OPS_ARG}")
if [[ "${OPS_ARG}" == "INVALID" ]]; then
  echo "Error: ops must be a valid hexadecimal value (e.g. 0x100000)" >&2
  exit 1
fi

mkdir -p "${ASSETS_DIR}"

CLEANUP_CALLED=0

cleanup() {
  if [[ ${CLEANUP_CALLED} -eq 1 ]]; then
    return
  fi
  CLEANUP_CALLED=1

  echo "=== [Phase 12/13] Executing guaranteed infrastructure teardown ===" >&2
  cd "${BENCHER_DIR}"
  terraform destroy -auto-approve || true

  if [[ -n "${INSTANCE_ID:-}" ]]; then
    echo "=== [Phase 13/13] Validating EC2 termination status ===" >&2
    local state
    state=$(aws ec2 describe-instances --region "${AWS_REGION}" \
      --instance-ids "${INSTANCE_ID}" \
      --query "Reservations[0].Instances[0].State.Name" \
      --output text 2>/dev/null || echo "terminated")
    echo "Instance [${INSTANCE_ID}] final state: ${state}" >&2
    if [[ "${state}" != "shutting-down" && "${state}" != "terminated" ]]; then
      echo "WARNING: Instance ${INSTANCE_ID} not yet terminated; current state: ${state}" >&2
    else
      echo "Validation passed: EC2 instance is terminated" >&2
    fi
  fi

  rm -f "${SSH_KEY}"
}

trap cleanup EXIT INT TERM

echo "=== [Phase 1/13] Validating local dependencies ===" >&2
for cmd in terraform aws ssh scp python3; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "Error: required command '${cmd}' not found" >&2
    exit 1
  fi
done
echo "Validation passed: required local tooling is present" >&2

echo "=== [Phase 2/13] Validating AWS authentication ===" >&2
CALLER_ARN=$(aws sts get-caller-identity --query "Arn" --output text)
echo "Validation passed: authenticated as ${CALLER_ARN}" >&2

echo "=== [Phase 3/13] Initializing and validating Terraform module ===" >&2
cd "${BENCHER_DIR}"
terraform init -upgrade
terraform validate
echo "Validation passed: Terraform configuration is valid" >&2

echo "=== [Phase 4/13] Provisioning EC2 infrastructure via Terraform ===" >&2
terraform apply -auto-approve -var="aws_region=${AWS_REGION}"

echo "=== [Phase 5/13] Extracting outputs and configuring SSH credentials ===" >&2
INSTANCE_ID=$(terraform output -raw instance_id)
PUBLIC_IP=$(terraform output -raw instance_public_ip)
terraform output -raw ssh_private_key_pem > "${SSH_KEY}"
chmod 0600 "${SSH_KEY}"

echo "Instance ID: ${INSTANCE_ID}" >&2
echo "Public IP  : ${PUBLIC_IP}" >&2

echo "=== [Phase 6/13] Validating EC2 instance state via AWS CLI ===" >&2
INSTANCE_STATE=""
for attempt in $(seq 1 0x1E); do
  INSTANCE_STATE=$(aws ec2 describe-instances --region "${AWS_REGION}" \
    --instance-ids "${INSTANCE_ID}" \
    --query "Reservations[0].Instances[0].State.Name" \
    --output text 2>/dev/null || echo "pending")
  if [[ "${INSTANCE_STATE}" == "running" ]]; then
    break
  fi
  echo "Waiting for instance to enter running state (current: ${INSTANCE_STATE})..." >&2
  sleep 2
done

if [[ "${INSTANCE_STATE}" != "running" ]]; then
  echo "Error: Instance failed to reach running state; current: ${INSTANCE_STATE}" >&2
  exit 1
fi
echo "Validation passed: EC2 instance is running" >&2

echo "=== [Phase 7/13] Validating SSH connectivity and cloud-init readiness ===" >&2
SSH_OPTS=(-i "${SSH_KEY}" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=120)

SSH_READY=0
for attempt in $(seq 1 0x3C); do
  if ssh "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" "true" >/dev/null 2>&1; then
    SSH_READY=1
    break
  fi
  echo "Waiting for SSH availability (attempt ${attempt}/0x3C)..." >&2
  sleep 3
done

if [[ ${SSH_READY} -ne 1 ]]; then
  echo "Error: Timed out waiting for SSH reachability on ${PUBLIC_IP}" >&2
  exit 1
fi
echo "Validation passed: SSH connection established" >&2

echo "=== [Phase 8/13] Synchronizing benchmark bootstrapper to remote host ===" >&2
scp "${SSH_OPTS[@]}" "${ROOT_DIR}/scripts/init_ec2_bench.sh" \
  "ubuntu@${PUBLIC_IP}:/tmp/init_ec2_bench.sh"
ssh "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" "chmod +x /tmp/init_ec2_bench.sh"
echo "Validation passed: bootstrapper synchronized and marked executable" >&2

echo "=== [Phase 9/13] Executing remote benchmark suite ===" >&2
ssh -t "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" \
  "/tmp/init_ec2_bench.sh --run --branch '${BRANCH_ARG}' --benches '${BENCHES_ARG}' \
  --scale-ops '${SCALE_OPS_ARG}' --ops '${OPS_ARG}' --out /tmp/results.json"
echo "Validation passed: remote benchmark suite finished successfully" >&2

echo "=== [Phase 10/13] Downloading and validating benchmark telemetry ===" >&2
scp "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}:/tmp/results.json" "${OUTPUT_JSON}"

if [[ ! -s "${OUTPUT_JSON}" ]]; then
  echo "Error: Downloaded telemetry file is empty or missing" >&2
  exit 1
fi

python3 -m json.tool "${OUTPUT_JSON}" > /dev/null
echo "Validation passed: Telemetry JSON schema is valid; written to ${OUTPUT_JSON}" >&2

echo "=== [Phase 11/13] Generating publication SVG assets from telemetry ===" >&2
GEN_SCRIPT="${ROOT_DIR}/scripts/generate_all_assets.py"
if [[ ! -f "${GEN_SCRIPT}" ]]; then
  echo "Error: asset generator script not found at ${GEN_SCRIPT}" >&2
  exit 1
fi

python3 "${GEN_SCRIPT}" "${OUTPUT_JSON}" "${ASSETS_DIR}"
echo "Validation passed: Benchmark SVG assets generated in ${ASSETS_DIR}" >&2

echo "All benchmark phases completed; Proceeding to automated teardown" >&2
