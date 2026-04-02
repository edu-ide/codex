# Codex-RS Execution Policy Reference

The Execution Policy defines the security boundaries for agent-initiated operations. It governs which commands can be run, which network hosts can be accessed, and whether the user must be prompted for approval.

## Decision Types

- **`Allow`**: The operation proceeds without user intervention.
- **`Prompt`**: (Default) The agent must request explicit user permission before proceeding.
- **`Forbidden`**: The operation is blocked immediately.

## Rule Categories

### Command Prefix Rules
- **Description**: Matches command-line invocations by their prefix.
- **Format**: `[program, arg1, arg2, ...]`
- **Example**: `["npm", "install"]` can be set to `Allow` to speed up dependency management.

### Network Rules
- **Description**: Restricts outgoing network requests based on host and protocol.
- **Supported Protocols**: `HTTPS`, `TCP`, `UDP`.
- **Wildcards**: Prefix/Suffix matching for domain names.

---

### Host Executable Resolution
Codex-RS can resolve symlinks or PATH-relative executables to their absolute paths on the host to ensure policies are applied to the actual binary being executed, preventing "Shadowing" attacks.

## Policy Layering
Policies are merged in a specific order:
1. **System Default**: Hardcoded safe-defaults.
2. **Global Config**: User's `~/.codex/policy.json`.
3. **Project Local**: `.codex/policy.json` within the current repository.
4. **Session Overlay**: Temporary permissions granted during a specific chat thread.

---

## Conflict Resolution
When multiple rules match a single operation, the **most restrictive** decision (Forbidden > Prompt > Allow) typically wins, unless an explicit `Allow` override is defined in a higher-priority layer.
