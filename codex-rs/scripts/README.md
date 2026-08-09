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

Keep the controller bound to loopback. Set one OpenSSH host alias in the local profile; Ilhae resolves its user, address, key, and SSH port through `~/.ssh/config`, opens the loopback tunnel automatically, and derives the inference and control URLs. `ssh_local_port` (default `18083`) and `ssh_remote_port` (default `8083`) are optional overrides.

The local profile keeps the runtime host's complete execution specification. `args`, `env`, paths, provider, logging, and startup timeout are sent to the controller; only `query_params` become inference-request query parameters. The runtime's own base and health URLs are derived from `--host` and `--port`, so they do not need to be repeated:

```toml
[profiles.fable.native_runtime]
enabled = true
provider = "llama-server"
ssh_host = "yth"
server_bin = "/opt/llama.cpp/bin/llama-server"
model_path = "/models/fable.gguf"
chat_template_file = "/home/yth/.ilhae/chat-templates/qwen.jinja"
log_file = "/home/yth/.ilhae/logs/fable.log"
startup_timeout_secs = 300
args = ["-m", "/models/fable.gguf", "-c", "131072", "-ngl", "100", "--port", "8081"]
```

Set `enabled = false` with `ssh_host` present to connect to an already-managed runtime without spawning or stopping it. Existing `health_url`, `base_url`, `proxy_base_url`, and `proxy_control_url` settings remain supported as explicit overrides. The inference proxy streams request and response bodies and forwards every HTTP path. Control payloads and inference traffic do not add proxy-specific size caps; operating-system process limits still apply when spawning a managed runtime. Upstream targets remain loopback-only by default. Set `ILHAE_RUNTIME_PROXY_ALLOW_NON_LOOPBACK_UPSTREAM=1` on the runtime host only when the controller must intentionally reach another HTTP(S) host.

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
