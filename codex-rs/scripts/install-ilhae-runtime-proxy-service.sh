#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="ilhae-runtime-proxy.service"
SERVICE_FILE="${SCRIPT_DIR}/systemd/${SERVICE_NAME}"
ENV_TEMPLATE="${SCRIPT_DIR}/systemd/${SERVICE_NAME%.service}.env"
BINARY="${SCRIPT_DIR}/../target/debug/ilhae-runtime-proxy"
SYSTEMD_DIR="/etc/systemd/system"
ENV_FILE="/etc/default/${SERVICE_NAME%.service}"
BINARY_DESTINATION="/usr/local/bin/ilhae-runtime-proxy"
ENABLE_SERVICE=0
START_SERVICE=0
OVERWRITE=0
DRY_RUN=0

usage() {
  cat <<'USAGE'
Usage: install-ilhae-runtime-proxy-service.sh [options]

Install the authenticated Ilhae runtime controller and streaming proxy.

Options:
  --binary PATH            Built ilhae-runtime-proxy binary
  --binary-destination PATH
                           Installed binary path (default: /usr/local/bin/ilhae-runtime-proxy)
  --systemd-dir PATH       Destination systemd directory (default: /etc/systemd/system)
  --env-file PATH          Destination env file (default: /etc/default/ilhae-runtime-proxy)
  --env-template PATH      Environment template to install
  --enable                 Enable service on boot
  --start                  Start/restart service after installation
  --overwrite              Overwrite existing unit, env file, and binary
  --dry-run                Print actions without applying them
  -h, --help               Show this help
USAGE
}

run_cmd() {
  if (( DRY_RUN )); then
    echo "+ $*"
    return 0
  fi
  "$@"
}

while (("$#")); do
  case "$1" in
    --binary)
      BINARY="${2:?missing binary path}"
      shift 2
      ;;
    --binary-destination)
      BINARY_DESTINATION="${2:?missing binary destination path}"
      shift 2
      ;;
    --systemd-dir)
      SYSTEMD_DIR="${2:?missing systemd-dir path}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:?missing env file path}"
      shift 2
      ;;
    --env-template)
      ENV_TEMPLATE="${2:?missing env template path}"
      shift 2
      ;;
    --enable)
      ENABLE_SERVICE=1
      shift
      ;;
    --start)
      START_SERVICE=1
      shift
      ;;
    --overwrite)
      OVERWRITE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if (( EUID != 0 )); then
  echo "This installer writes to system directories. Run as root or with sudo." >&2
  exit 1
fi

for required in "$SERVICE_FILE" "$ENV_TEMPLATE" "$BINARY"; do
  if [[ ! -f "$required" ]]; then
    echo "Missing required file: $required" >&2
    exit 1
  fi
done

install_file() {
  local source="$1"
  local destination="$2"
  local mode="$3"
  if [[ -e "$destination" && "$OVERWRITE" != 1 ]]; then
    echo "Destination exists and --overwrite is not set: $destination"
    return
  fi
  run_cmd install -Dm"$mode" "$source" "$destination"
}

install_file "$BINARY" "$BINARY_DESTINATION" 0755
install_file "$SERVICE_FILE" "${SYSTEMD_DIR%/}/$SERVICE_NAME" 0644
install_file "$ENV_TEMPLATE" "$ENV_FILE" 0600
run_cmd systemctl daemon-reload

if (( ENABLE_SERVICE )); then
  run_cmd systemctl enable "$SERVICE_NAME"
fi
if (( START_SERVICE )); then
  run_cmd systemctl restart "$SERVICE_NAME"
fi
