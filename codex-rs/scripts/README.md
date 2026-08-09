# Codex service scripts

Install and enable the remote-control systemd unit:

```bash
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sudo "${SCRIPT_DIR}/install-codex-llama-server-service.sh" --overwrite --enable --start
```

Optional:
```bash
sudo "${SCRIPT_DIR}/install-codex-llama-server-service.sh" --overwrite --start \
  --env-template "${SCRIPT_DIR}/systemd/codex-llama-server.env"
sudo "${SCRIPT_DIR}/install-codex-llama-server-service.sh" --overwrite --start \
  --env-file /etc/default/codex-llama-server
```

Build, install, and enable the native-runtime control proxy on the runtime host:

```bash
cd /home/yth/codex/codex-rs
cargo build -p codex-ilhae --bin ilhae-runtime-proxy
sudo scripts/install-ilhae-runtime-proxy-service.sh --overwrite --enable --start
```

Keep the controller bound to loopback and carry both control and inference traffic through SSH:

```bash
ssh -N -L 18083:127.0.0.1:8083 yth
```

The local profile keeps the runtime host's complete execution specification. `args`, `env`, paths, provider, logging, and startup timeout are sent to the controller; only `query_params` become inference-request query parameters:

```toml
[profiles.fable.native_runtime]
enabled = false
provider = "llama-server"
health_url = "http://127.0.0.1:8081/health"
base_url = "http://127.0.0.1:8081/v1"
proxy_base_url = "http://127.0.0.1:18083/v1"
proxy_control_url = "http://127.0.0.1:18083/_ilhae/native-runtime/ensure"
server_bin = "/opt/llama.cpp/bin/llama-server"
model_path = "/models/fable.gguf"
chat_template_file = "/home/yth/.ilhae/chat-templates/qwen.jinja"
log_file = "/home/yth/.ilhae/logs/fable.log"
startup_timeout_secs = 300
args = ["-m", "/models/fable.gguf", "-c", "131072", "-ngl", "100", "--port", "8081"]
```

Set `enabled = false` with the proxy fields present to connect to an already-managed runtime without spawning or stopping it. The inference proxy streams request and response bodies and forwards every HTTP path. Control payloads and inference traffic do not add proxy-specific size caps; operating-system process limits still apply when spawning a managed runtime. Upstream targets remain loopback-only by default. Set `ILHAE_RUNTIME_PROXY_ALLOW_NON_LOOPBACK_UPSTREAM=1` on the runtime host only when the controller must intentionally reach another HTTP(S) host.

If the proxy is exposed without an SSH tunnel, put it behind TLS, set `ILHAE_RUNTIME_PROXY_TOKEN` in `/etc/default/ilhae-runtime-proxy`, and set `proxy_control_token_env` to the local environment variable containing the same token. Non-loopback binding without a token is rejected; do not send the token over plain HTTP.

Check both services:

Service check:
```bash
systemctl --no-pager status codex-llama-server
systemctl --no-pager status ilhae-runtime-proxy
systemctl stop codex-llama-server
systemctl start codex-llama-server
systemctl stop ilhae-runtime-proxy
systemctl start ilhae-runtime-proxy
```
