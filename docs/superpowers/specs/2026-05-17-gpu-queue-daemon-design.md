# GPU Queue Daemon Design

Date: 2026-05-17

## Summary

Add a single local GPU queue daemon that owns GPU scheduling and local LLM runtime
preemption. Codex/ilhae, MCP tools, shell scripts, and external apps such as
`/mnt/sda1/projects/apps/videoeditor` must all use this same daemon instead of
stopping `llama-server` or starting ComfyUI independently.

The daemon exposes three front doors:

- Local HTTP API for external apps and long-running services.
- CLI subcommands for humans and shell scripts.
- Ilhae MCP built-in tools for agent-initiated GPU work.

All three front doors are thin clients. The daemon is the only owner of queue
state, leases, LLM stop/start decisions, and crash recovery.

## Goals

- Allow a single RTX 4090 to serve local LLM agent work most of the time.
- When a video/image generation job needs the GPU exclusively, stop the local
  LLM runtime, wait for VRAM to clear, run the job, then restart the LLM runtime.
- Let Codex/ilhae and external apps use the same queue safely.
- Prevent race conditions between CLI, MCP tools, videoeditor, and manual
  scripts.
- Keep the implementation out of `codex-core`; place the feature in
  `codex-rs/ilhae` and reuse existing native runtime start/stop helpers.

## Non-goals

- Distributed GPU scheduling across machines.
- Fine-grained VRAM paging inside ComfyUI, Wan2GP, or llama.cpp.
- Multi-GPU placement logic.
- Replacing ComfyUI's own prompt queue.
- Persisting arbitrary job payload outputs in Ilhae storage. The caller remains
  responsible for its own artifacts.

## Architecture

Add a new `codex-rs/ilhae/src/gpu_queue/` module with focused submodules:

- `daemon`: local HTTP server, lifecycle, and shutdown handling.
- `scheduler`: FIFO lease queue, active lease tracking, priority hooks.
- `llm_runtime`: adapter over existing native runtime start/stop/status helpers.
- `vram`: `nvidia-smi` based VRAM polling and process observation.
- `api`: request/response types shared by CLI and server.
- `client`: HTTP client used by CLI and MCP tools.
- `mcp`: Ilhae built-in tool registration helpers.

The daemon binds to localhost by default. The first implementation should use a
loopback TCP port because videoeditor is a Node app and HTTP integration is
simple. Unix socket support is out of scope for the initial implementation.

Default listen address:

```text
127.0.0.1:43290
```

This address should be configurable from Ilhae config and environment variables.

## API

Initial local HTTP API:

```text
GET  /health
GET  /status
POST /leases
POST /leases/{lease_id}/heartbeat
POST /leases/{lease_id}/release
POST /jobs/run
POST /llm/start
POST /llm/stop
POST /llm/restart
```

`POST /leases` request:

```json
{
  "owner": "videoeditor",
  "kind": "video",
  "mode": "exclusive",
  "preemptLlm": true,
  "ttlSeconds": 3600
}
```

`POST /leases` response when granted:

```json
{
  "leaseId": "gpu-lease-...",
  "state": "granted",
  "llmWasPreempted": true
}
```

The API should also support a blocking wait with timeout so simple callers do not
need to poll:

```json
{
  "owner": "videoeditor",
  "kind": "video",
  "mode": "exclusive",
  "preemptLlm": true,
  "ttlSeconds": 3600,
  "waitTimeoutSeconds": 900
}
```

`GET /status` response includes:

- Daemon uptime and version.
- LLM runtime state: `running`, `stopped`, `starting`, `stopping`, `unknown`.
- Active lease, if any.
- Pending leases.
- Last preemption and restart errors.
- GPU memory snapshot when `nvidia-smi` is available.

## CLI

Add feature-gated Codex/Ilhae subcommands:

```text
codex gpu daemon
codex gpu status
codex gpu acquire --kind video --exclusive --preempt-llm
codex gpu release <lease-id>
codex gpu run --kind video --exclusive --preempt-llm -- <command...>
codex gpu llm start
codex gpu llm stop
codex gpu llm restart
```

If an `ilhae` binary dispatch already exists in the installed environment,
expose the same surface as an alias. The required implementation target is the
feature-gated `codex gpu` command.

```text
ilhae gpu status
ilhae gpu run --kind video --exclusive --preempt-llm -- <command...>
```

`gpu run` should acquire a lease, execute the child command, stream stdout/stderr,
release the lease on normal exit, and release on signal/interrupt where possible.

## MCP Tools

Add Ilhae built-in MCP tools that call the daemon API:

- `gpu_status`
- `gpu_acquire`
- `gpu_release`
- `gpu_run_command`
- `gpu_llm_start`
- `gpu_llm_stop`
- `gpu_llm_restart`

Agent-facing generation tools must not directly kill `llama-server`. They should
acquire an exclusive lease with `preemptLlm: true`, perform generation work, then
release the lease.

## Videoeditor Integration

`/mnt/sda1/projects/apps/videoeditor` should integrate through the HTTP API or a
small Node client wrapper. Before high-VRAM ComfyUI or Wan2GP work, it should:

1. Request an exclusive lease with `preemptLlm: true`.
2. Run the existing ComfyUI workflow or service call.
3. Heartbeat during long jobs.
4. Release the lease when the job finishes or fails.

The existing `scripts/manage_stack.sh` can also be wrapped:

```bash
codex gpu run --kind video --exclusive --preempt-llm -- \
  bash /mnt/sda1/projects/apps/videoeditor/scripts/manage_stack.sh start comfy
```

Longer term, direct code integration in `comfyui-service.ts` is cleaner because
it can keep the lease active around the exact generation window instead of the
whole app stack lifecycle.

## LLM Runtime Behavior

When granting an exclusive lease with `preemptLlm: true`:

1. Check current native runtime health.
2. If healthy, stop the active native runtime profile using existing Ilhae
   runtime stop logic.
3. Poll VRAM until memory drops below a configurable threshold or timeout.
4. Mark the lease granted.
5. On release or TTL expiry, restart the native runtime profile if the daemon
   preempted it.
6. Wait for healthcheck before reporting LLM runtime as ready.

The daemon must remember whether it stopped the LLM. If the LLM was already
stopped before the lease, releasing the lease must not start it unexpectedly.

## Failure Handling

- Lease TTL is required for exclusive leases.
- Callers can extend the lease with heartbeat.
- If the caller dies and heartbeats stop, the daemon expires the lease and
  restarts the LLM if it preempted it.
- If LLM stop fails, the lease request fails unless the request explicitly allows
  best-effort preemption.
- If LLM restart fails after job completion, the daemon records the error in
  status and returns it to the releasing caller.
- If VRAM does not clear by timeout, the lease request fails and the daemon
  attempts to restore the previous LLM state.
- `gpu run` must release the lease even when the child exits with a non-zero
  status.

## Configuration

Add Ilhae GPU queue config without changing `codex-core`:

```toml
[gpu_queue]
enabled = true
listen = "127.0.0.1:43290"
default_lease_ttl_secs = 3600
vram_clear_timeout_secs = 120
vram_clear_threshold_mb = 2048
restart_llm_after_exclusive = true
```

The daemon should also accept environment overrides for local development:

```text
ILHAE_GPU_QUEUE_ADDR
ILHAE_GPU_QUEUE_DEFAULT_TTL_SECS
ILHAE_GPU_QUEUE_VRAM_CLEAR_THRESHOLD_MB
```

## Testing

Unit tests:

- Scheduler grants one exclusive lease at a time.
- Pending leases are FIFO by default.
- TTL expiry releases active lease and schedules LLM restart if needed.
- Releasing a lease not owned by the daemon returns a clear error.
- LLM restart only happens when the daemon preempted the LLM.

Integration tests with fake runtime:

- Exclusive lease preempts fake LLM, grants lease, and restarts fake LLM on
  release.
- `gpu run` releases the lease on child failure.
- API status shows active and pending leases.

Manual validation:

- `codex gpu daemon`
- `codex gpu status`
- `codex gpu run --kind video --exclusive --preempt-llm -- sleep 5`
- videoeditor lease request against the local API.

## Rollout

1. Implement daemon, API types, and scheduler with fake LLM runtime tests.
2. Wire native runtime start/stop through an adapter.
3. Add CLI subcommands.
4. Add Ilhae MCP tools.
5. Add a videoeditor HTTP client wrapper in a separate change.
6. Add docs for `codex gpu` usage and videoeditor integration.
