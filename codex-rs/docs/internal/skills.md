# Codex-RS Skills Reference

Skills are sets of instructions, scripts, and resources that extend Codex-RS's capabilities for specialized tasks.

## Built-in Skills

### `skill-creator`
- **Purpose**: Automates the creation of new Codex-RS skills.
- **Capabilities**: Generates `SKILL.md` templates and boilerplate scripts.
- **Usage**: Invoke via `/skills use skill-creator` to bootstrap a new capability.

### `plugin-creator`
- **Purpose**: Scaffold new MCP-compatible plugins.
- **Capabilities**: Creates `plugin.json` and basic directory structures.

---

### `imagegen`
- **Purpose**: High-fidelity image generation integration.
- **Capabilities**: Provides tool Access to DALL-E or Stable Diffusion gateways.
- **Usage**: Useful for UI/UX prototyping missions.

---

### `openai-docs`
- **Purpose**: Contextual knowledge for OpenAI API development.
- **Capabilities**: Pre-indexed documentation for GPT-4, Embeddings, and Moderation APIs.

## Custom Skills
Users can define their own skills in the `.codex/skills/` directory. Each skill MUST contain:
- `SKILL.md`: Root instruction file with YAML metadata.
- `scripts/`: Optional helper utilities.
