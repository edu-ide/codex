#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="codex-llama-server-proxy.service"
SERVICE_FILE="${SCRIPT_DIR}/systemd/${SERVICE_NAME}"
CONF_TEMPLATE="${SCRIPT_DIR}/systemd/codex-llama-server-proxy.conf.template"
ENV_TEMPLATE="${SCRIPT_DIR}/systemd/${SERVICE_NAME%.service}.env"

SYSTEMD_DIR="/etc/systemd/system"
ENV_FILE="/etc/default/${SERVICE_NAME%.service}"
CONF_DIR="/etc/codex"
CONF_FILE="${CONF_DIR}/codex-llama-server-proxy.conf"

ENABLE_SERVICE=0
START_SERVICE=0
OVERWRITE=0
DRY_RUN=0
DEFAULT_LLAMA_PROXY_ENV_VARS='${LLAMA_UPSTREAM_HOST} ${LLAMA_UPSTREAM_PORT} ${LLAMA_PROXY_LISTEN_ADDR} ${LLAMA_PROXY_LISTEN_PORT} ${LLAMA_PROXY_KEEPALIVE_TIMEOUT} ${LLAMA_PROXY_KEEPALIVE_REQUESTS} ${LLAMA_PROXY_READ_TIMEOUT} ${LLAMA_PROXY_CLIENT_MAX_BODY_SIZE} ${LLAMA_PROXY_CORS_ORIGINS} ${LLAMA_PROXY_RUNTIME_USER} ${LLAMA_PROXY_DEFAULT_QUERY_ARGS}'

require_env_token() {
  local list="$1"
  local token="$2"
  if [[ " $list " == *" $token "* ]]; then
    echo "$list"
    return
  fi
  if [[ -n "$list" ]]; then
    echo "$list $token"
  else
    echo "$token"
  fi
}

ensure_llama_proxy_env_vars() {
  local vars="$1"
  local token
  local required_tokens=(
    '${LLAMA_UPSTREAM_HOST}'
    '${LLAMA_UPSTREAM_PORT}'
    '${LLAMA_PROXY_LISTEN_ADDR}'
    '${LLAMA_PROXY_LISTEN_PORT}'
    '${LLAMA_PROXY_KEEPALIVE_TIMEOUT}'
    '${LLAMA_PROXY_KEEPALIVE_REQUESTS}'
    '${LLAMA_PROXY_READ_TIMEOUT}'
    '${LLAMA_PROXY_CLIENT_MAX_BODY_SIZE}'
    '${LLAMA_PROXY_CORS_ORIGINS}'
    '${LLAMA_PROXY_RUNTIME_USER}'
    '${LLAMA_PROXY_DEFAULT_QUERY_ARGS}'
  )
  for token in "${required_tokens[@]}"; do
    vars="$(require_env_token "$vars" "$token")"
  done
  echo "$vars"
}

parse_host_port_from_url() {
  local input="$1"
  local host_ref="$2"
  local port_ref="$3"
  local host=""
  local port="80"
  local no_path

  local no_scheme="${input#*://}"
  no_path="${no_scheme%%/*}"

  if [[ "$no_path" == \[* ]]; then
    host="${no_path#\[}"
    host="${host%]*}"
    local rest="${no_path##*\]}"
    if [[ "$rest" == :* ]]; then
      port="${rest#:}"
    fi
  elif [[ "$no_path" == *:* ]]; then
    host="${no_path%:*}"
    port="${no_path##*:}"
  else
    host="$no_path"
  fi

  printf -v "$host_ref" '%s' "$host"
  printf -v "$port_ref" '%s' "$port"
}

usage() {
  cat <<'USAGE'
Usage: install-codex-llama-server-proxy-service.sh [options]

Install nginx-based proxy systemd service and generate its config.

Options:
  --systemd-dir PATH       Destination systemd directory (default: /etc/systemd/system)
  --env-file PATH          Destination env file path (default: /etc/default/codex-llama-server-proxy)
  --conf-file PATH         Destination nginx conf path (default: /etc/codex/codex-llama-server-proxy.conf)
  --service-template PATH  Optional custom service unit template path
                          (default: systemd/codex-llama-server-proxy.service)
  --env-template PATH      Optional custom env template path
                          (default: systemd/codex-llama-server-proxy.env)
  --conf-template PATH     Optional custom nginx conf template path
                          (default: systemd/codex-llama-server-proxy.conf.template)
  --enable                 Enable service on boot
  --start                  Start/restart service after installation
  --overwrite              Overwrite existing unit/env/conf files
  --dry-run                Print actions without applying
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

while (($#)); do
  case "$1" in
    --systemd-dir)
      SYSTEMD_DIR="${2:?missing systemd-dir path}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:?missing env-file path}"
      shift 2
      ;;
    --conf-file)
      CONF_FILE="${2:?missing conf-file path}"
      shift 2
      ;;
    --service-template)
      SERVICE_FILE="${2:?missing service-template path}"
      shift 2
      ;;
    --env-template)
      ENV_TEMPLATE="${2:?missing env-template path}"
      shift 2
      ;;
    --conf-template)
      CONF_TEMPLATE="${2:?missing conf-template path}"
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

if ! command -v envsubst >/dev/null 2>&1; then
  echo "Missing envsubst (gettext). Install gettext package first." >&2
  exit 1
fi

if [[ ! -f "${SERVICE_FILE}" ]]; then
  echo "Missing service template: ${SERVICE_FILE}" >&2
  exit 1
fi

if [[ ! -f "${ENV_TEMPLATE}" ]]; then
  echo "Missing env template: ${ENV_TEMPLATE}" >&2
  exit 1
fi

if [[ ! -f "${CONF_TEMPLATE}" ]]; then
  echo "Missing nginx conf template: ${CONF_TEMPLATE}" >&2
  exit 1
fi

if [[ ! -x "$(command -v nginx)" ]]; then
  echo "nginx binary not found. Install nginx first." >&2
  exit 1
fi

target_unit="${SYSTEMD_DIR%/}/${SERVICE_NAME}"

install_file() {
  local src="$1"
  local dst="$2"
  local label="$3"
  if [[ -e "$dst" && "$OVERWRITE" != 1 ]]; then
    echo "${label} already exists and --overwrite is not set: $dst"
    return
  fi
  run_cmd install -Dm0644 "$src" "$dst"
}

mkdir -p "${CONF_DIR}"
install_file "${SERVICE_FILE}" "$target_unit" "service unit"
install_file "${ENV_TEMPLATE}" "$ENV_FILE" "environment file"
install_file "${CONF_TEMPLATE}" "${CONF_FILE}.template" "nginx conf template"

if [[ -r "$ENV_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$ENV_FILE"
fi

if [[ -n "${LLAMA_UPSTREAM_URL:-}" ]]; then
  parse_host_port_from_url "$LLAMA_UPSTREAM_URL" LLAMA_UPSTREAM_HOST LLAMA_UPSTREAM_PORT
fi

if [[ -n "${LLAMA_PROXY_LISTEN_URL:-}" ]]; then
  parse_host_port_from_url "$LLAMA_PROXY_LISTEN_URL" LLAMA_PROXY_LISTEN_ADDR LLAMA_PROXY_LISTEN_PORT
  LLAMA_PROXY_LISTEN_ADDR="${LLAMA_PROXY_LISTEN_ADDR#[}"
  LLAMA_PROXY_LISTEN_ADDR="${LLAMA_PROXY_LISTEN_ADDR%\]}"
fi

LLAMA_PROXY_CONF_TEMPLATE="${LLAMA_PROXY_CONF_TEMPLATE:-${CONF_FILE}.template}"
LLAMA_PROXY_CONF_FILE="${LLAMA_PROXY_CONF_FILE:-${CONF_FILE}}"
LLAMA_PROXY_ENV_VARS="$(ensure_llama_proxy_env_vars "${LLAMA_PROXY_ENV_VARS:-$DEFAULT_LLAMA_PROXY_ENV_VARS}")"

if [[ ! -f "$LLAMA_PROXY_CONF_TEMPLATE" ]]; then
  echo "Missing rendered nginx conf template: $LLAMA_PROXY_CONF_TEMPLATE" >&2
  exit 1
fi

# Render the effective nginx config under /etc so systemd can run with a static file.
run_cmd bash -c \
  'envsubst "$0" < "$1" > "$2"' \
  "$LLAMA_PROXY_ENV_VARS" \
  "$LLAMA_PROXY_CONF_TEMPLATE" \
  "$LLAMA_PROXY_CONF_FILE"

run_cmd systemctl daemon-reload

if (( ENABLE_SERVICE )); then
  run_cmd systemctl enable "${SERVICE_NAME}"
fi

if (( START_SERVICE )); then
  run_cmd systemctl restart "${SERVICE_NAME}"
fi
