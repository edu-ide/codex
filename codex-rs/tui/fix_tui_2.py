import re

# 1. app_server_session.rs
with open("src/app_server_session.rs", "r") as f:
    data = f.read()

data = re.sub(
    r"(ModelPreset\s*\{[\s\S]*?)(max_tokens:.*?,\n)",
    r"\1\2                additional_speed_tiers: Vec::new(),\n",
    data
)

data = re.sub(
    r"(Turn\s*\{[\s\S]*?)(message:\s*(?:None|Some\(.*?\)),\n)",
    r"\1\2                        started_at: None,\n                        completed_at: None,\n                        duration_ms: None,\n",
    data
)

with open("src/app_server_session.rs", "w") as f:
    f.write(data)

# 2. chatwidget/slash_dispatch.rs
with open("src/chatwidget/slash_dispatch.rs", "r") as f:
    data = f.read()

data = data.replace('add_error_message("✅ Dream mode enabled.")', 'add_error_message("✅ Dream mode enabled.".to_string())')
data = data.replace('add_error_message("🚫 Dream mode disabled.")', 'add_error_message("🚫 Dream mode disabled.".to_string())')
data = data.replace('add_error_message("✅ Embed mode enabled.")', 'add_error_message("✅ Embed mode enabled.".to_string())')
data = data.replace('add_error_message("🚫 Embed mode disabled.")', 'add_error_message("🚫 Embed mode disabled.".to_string())')

with open("src/chatwidget/slash_dispatch.rs", "w") as f:
    f.write(data)

# 3. chatwidget.rs
with open("src/chatwidget.rs", "r") as f:
    data = f.read()

data = re.sub(
    r"(ModelPreset\s*\{[\s\S]*?)(max_tokens:.*?,\n)",
    r"\1\2                additional_speed_tiers: Vec::new(),\n",
    data
)

with open("src/chatwidget.rs", "w") as f:
    f.write(data)

# 4. slash_command.rs
with open("src/slash_command.rs", "r") as f:
    data = f.read()

data = data.replace(" | SlashCommand::Help =>", " =>")

with open("src/slash_command.rs", "w") as f:
    f.write(data)

