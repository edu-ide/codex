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

Install and enable the local llama-server edge proxy:

```bash
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
sudo "${SCRIPT_DIR}/install-codex-llama-server-proxy-service.sh" --overwrite --enable --start
```

Optional (change listen/upstream with one URL, or legacy host/port, CORS, or timeouts):

```bash
sudo cp /home/yth/codex/codex-rs/scripts/systemd/codex-llama-server-proxy.env /etc/default/codex-llama-server-proxy
sudo sed -i 's|LLAMA_UPSTREAM_URL=.*|LLAMA_UPSTREAM_URL="http://127.0.0.1:8082"|' /etc/default/codex-llama-server-proxy
sudo sed -i 's|LLAMA_PROXY_LISTEN_URL=.*|LLAMA_PROXY_LISTEN_URL="http://0.0.0.0:8083"|' /etc/default/codex-llama-server-proxy
# Legacy host/port mode (optional, still supported):
# sudo sed -i 's/LLAMA_UPSTREAM_HOST=.*/LLAMA_UPSTREAM_HOST=127.0.0.1/' /etc/default/codex-llama-server-proxy
# sudo sed -i 's/LLAMA_UPSTREAM_PORT=.*/LLAMA_UPSTREAM_PORT=8082/' /etc/default/codex-llama-server-proxy
# sudo sed -i 's/LLAMA_PROXY_LISTEN_ADDR=.*/LLAMA_PROXY_LISTEN_ADDR=0.0.0.0/' /etc/default/codex-llama-server-proxy
# sudo sed -i 's/LLAMA_PROXY_LISTEN_PORT=.*/LLAMA_PROXY_LISTEN_PORT=8083/' /etc/default/codex-llama-server-proxy
sudo sed -i 's|LLAMA_PROXY_CORS_ORIGINS=.*|LLAMA_PROXY_CORS_ORIGINS="*"|' /etc/default/codex-llama-server-proxy
sudo sed -i 's|LLAMA_PROXY_DEFAULT_QUERY_ARGS=.*|LLAMA_PROXY_DEFAULT_QUERY_ARGS="draft=mtp&ngram-mode=1&cache-type-k=turbo4_0&cache-type-v=turbo4_0"|' /etc/default/codex-llama-server-proxy
sudo systemctl restart codex-llama-server-proxy
```

`LLAMA_PROXY_DEFAULT_QUERY_ARGS` is the default query fragment appended to every `/v1` request through the proxy:

- Set empty (`LLAMA_PROXY_DEFAULT_QUERY_ARGS=""`) to disable.
- Set query keys like `draft=mtp`, `ngram-mode=1`, `cache-type-k=turbo4_0`, `cache-type-v=turbo4_0` to expose tunings externally.
- `cache-type-*` must remain actual runtime keys; these are examples currently used for turbo-quant tuning.

Check both services:

Service check:
```bash
systemctl --no-pager status codex-llama-server
systemctl --no-pager status codex-llama-server-proxy
systemctl stop codex-llama-server
systemctl start codex-llama-server
systemctl stop codex-llama-server-proxy
systemctl start codex-llama-server-proxy
```
