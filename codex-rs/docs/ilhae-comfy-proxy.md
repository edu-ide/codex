# Ilhae ComfyUI GPU Proxy

`ilhae gpu comfy-proxy` exposes a ComfyUI-compatible HTTP API that routes GPU
prompt work through the Ilhae GPU queue. It is intended to sit in front of the
real ComfyUI backend:

- proxy: `http://127.0.0.1:8189`
- backend ComfyUI: `http://127.0.0.1:8188`
- GPU queue daemon: `http://127.0.0.1:43290`

Clients such as `videoeditor`, scripts, or direct ComfyUI API callers should
use the proxy URL. The proxy acquires an exclusive GPU lease for `POST /prompt`,
preempts the local LLM through the GPU queue daemon, forwards the prompt to
ComfyUI, watches `/history/{prompt_id}`, frees ComfyUI memory, stops ComfyUI if
configured, and finally releases the lease so the LLM can resume.

Non-prompt API calls are proxied normally. `GET /view` first tries to serve
`output`, `input`, and `temp` files directly from the ComfyUI directory, so
generated files remain reachable even when the backend has been stopped.

Example `~/.ilhae/config.toml`:

```toml
[comfy_proxy]
listen = "127.0.0.1:8189"
backend_url = "http://127.0.0.1:8188"
comfy_root = "/mnt/sda1/ComfyUI"
gpu_queue_addr = "127.0.0.1:43290"
owner = "comfyui-gateway"
start_command = "bash /mnt/sda1/projects/apps/videoeditor/scripts/manage_stack.sh start comfy"
stop_command = "bash /mnt/sda1/projects/apps/videoeditor/scripts/manage_stack.sh stop comfy"
ttl_seconds = 3600
wait_timeout_seconds = 900
prompt_poll_interval_ms = 2000
prompt_timeout_seconds = 3600
preempt_llm = true
stop_after_prompt = true
start_backend_for_passthrough = false
```

Run:

```bash
ilhae gpu daemon
ilhae gpu comfy-proxy
```

The daemon also exposes `GET /events` as `text/event-stream`. Ilhae app-server
instances bridge those GPU queue runtime events into the v2
`gpuQueue/runtimeEvent` notification so TUI clients can show when the local LLM
runtime is stopped for ComfyUI work and when it starts again.

Then point clients at the proxy:

```bash
COMFYUI_API_URL=http://127.0.0.1:8189
```

Clients that already acquire Ilhae GPU leases around ComfyUI calls should turn
that local lease wrapper off before switching to the proxy, otherwise a prompt
can acquire a lease in the client and then wait for a second exclusive lease in
the proxy. For `videoeditor`, use the proxy as the ComfyUI API endpoint and let
the proxy own ComfyUI process management:

```bash
COMFYUI_API_URL=http://127.0.0.1:8189
VIDEOEDITOR_GPU_QUEUE_ENABLED=false
VIDEOEDITOR_COMFYUI_MANAGED_PROCESS=false
```

With `preempt_llm = true`, the GPU queue waits for the local llama.cpp runtime
to report idle via `/slots` before stopping it and granting the ComfyUI lease.
This prevents the queue from killing the active TUI model stream mid-response.
The idle wait uses the lease wait timeout when one is present; otherwise it
defaults to 900 seconds. Override the fallback with
`ILHAE_GPU_QUEUE_LLM_IDLE_WAIT_SECONDS`.

Use `preempt_llm = false` only when TUI continuity is more important than
freeing VRAM automatically.
