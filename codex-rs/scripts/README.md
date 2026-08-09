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
sudo scripts/install-ilhae-runtime-proxy-service.sh \
  --service-user "$(id -un)" --overwrite --enable --start
```

For the current yth deployment, including the exact public endpoint, client
profile, token handoff, verification, and troubleshooting steps, see
[`ILHAE_RUNTIME_PROXY_YTH_HANDOFF.ko.md`](ILHAE_RUNTIME_PROXY_YTH_HANDOFF.ko.md).

Every llama-server profile uses the same topology: `Ilhae -> runtime proxy -> llama-server`. The profile's `native_runtime` table is the single source of truth for the complete runtime execution specification. `args`, `env`, paths, provider, logging, request headers, query parameters, retry policy, context window, and startup timeout work identically for local and remote profiles. Only `query_params` become inference-request query parameters.

For a local profile, omit `proxy_url`. Ilhae automatically uses the local proxy at `http://127.0.0.1:8083` and derives the inference, health, and control routes from that one origin:

```toml
[profiles.fable-local.native_runtime]
enabled = true
provider = "llama-server"
server_bin = "/opt/llama.cpp/bin/llama-server"
model_path = "/models/fable.gguf"
chat_template_file = "/home/me/.ilhae/chat-templates/qwen.jinja"
log_file = "/home/me/.ilhae/logs/fable.log"
startup_timeout_secs = 300
args = ["-m", "/models/fable.gguf", "-c", "131072", "-ngl", "100", "--host", "127.0.0.1", "--port", "8081"]
```

For a remote runtime, keep the same complete table and add only the authenticated proxy origin and token. The runtime's own base and health URLs are still derived from `--host` and `--port`, so they do not need to be repeated:

```toml
[profiles.fable.native_runtime]
enabled = true
provider = "llama-server"
proxy_url = "https://ilhae-runtime.example.com"
proxy_token = "replace-with-a-long-random-token"
server_bin = "/opt/llama.cpp/bin/llama-server"
model_path = "/models/fable.gguf"
chat_template_file = "/home/yth/.ilhae/chat-templates/qwen.jinja"
log_file = "/home/yth/.ilhae/logs/fable.log"
startup_timeout_secs = 300
args = ["-m", "/models/fable.gguf", "-c", "131072", "-ngl", "100", "--port", "8081"]
```

HTTPS is required away from loopback by default. A direct public-IP deployment without TLS must opt in on that profile; this sends the token, prompts, and responses without transport encryption:

```toml
proxy_url = "http://203.0.113.10:8083"
proxy_allow_insecure_http = true
proxy_token = "replace-with-the-installed-runtime-token"
```

Set `enabled = false` to connect through the same proxy to an already-managed runtime without spawning or stopping it. The inference proxy streams request and response bodies and forwards every HTTP path. Control payloads and inference traffic do not add proxy-specific size caps; operating-system process limits still apply when spawning a managed runtime. Upstream targets remain loopback-only by default. Set `ILHAE_RUNTIME_PROXY_ALLOW_NON_LOOPBACK_UPSTREAM=1` on the runtime host only when the controller must intentionally reach another HTTP(S) host.

The production service installer binds `0.0.0.0:8083` and generates a persistent random `ILHAE_RUNTIME_PROXY_TOKEN` in `/etc/default/ilhae-runtime-proxy`. Copy that token into authorized profiles. Existing environment files and tokens are preserved unless `--overwrite-env` is passed. Non-loopback binding without a token is rejected. A firewall can narrow which clients reach the listener, and HTTPS should be added before treating an Internet-facing deployment as transport-secure. nginx, Cloudflare, and SSH tunnels are not required by Ilhae; SSH may still be used separately for source deployment and administration.

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
