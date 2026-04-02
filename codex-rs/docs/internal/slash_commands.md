# Codex-RS Slash Commands

Codex-RS supports the following slash commands for management and interaction.

## General Commands

### `/help`
Displays a list of available commands and their descriptions.
- **Usage**: `/help [command]`

### `/recall`
Search and recall memories from previous sessions using FTS5.
- **Usage**: `/recall <query>`
- **Advanced**: Rank results based on relevance and chronological context.

### `/compact`
Compact the current session history by summarizing earlier interactions.
- **Usage**: `/compact`

---

## Automation & Skills

### `/schedule`
Create a scheduled task (Cron) using natural language.
- **Usage**: `/schedule "Run a daily audit at 9 PM"`
- **Backend**: Integrated with `brain_schedule_ops`.

### `/skill`
Manage and generate autonomous skills.
- **Usage**: `/skill list`, `/skill create <task_id>`
- **Logic**: Summarizes successful task patterns for future reuse.

---

## System Commands

### `/reset`
Resets the agent's internal state and context.
- **Usage**: `/reset`

### `/undo`
Rollback the last interaction or tool execution.
- **Usage**: `/undo`
