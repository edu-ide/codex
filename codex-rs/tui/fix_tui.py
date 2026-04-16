import re

# 1. app_server_session.rs
with open("src/app_server_session.rs", "r") as f:
    data = f.read()

data = data.replace(
    "                id: String::new(),\n            }",
    "                id: String::new(),\n                additional_speed_tiers: Vec::new(),\n            }"
)
data = data.replace(
    "                id: \"\".to_string(),\n            }",
    "                id: \"\".to_string(),\n                additional_speed_tiers: Vec::new(),\n            }"
)
data = re.sub(
    r"(turn:\s*Turn\s*\{[\s\S]*?)(message:\s*None,)",
    r"\1\2\n                        started_at: None,\n                        completed_at: None,\n                        duration_ms: None,",
    data
)
data = re.sub(
    r"(current_turn\s*=\s*Some\(Turn\s*\{[\s\S]*?)(message:\s*None,)",
    r"\1\2\n                                        started_at: None,\n                                        completed_at: None,\n                                        duration_ms: None,",
    data
)
data = re.sub(
    r"(let\s+initial_turn\s*=\s*Turn\s*\{[\s\S]*?)(message:\s*None,)",
    r"\1\2\n                            started_at: None,\n                            completed_at: None,\n                            duration_ms: None,",
    data
)

with open("src/app_server_session.rs", "w") as f:
    f.write(data)

# 2. chatwidget/slash_dispatch.rs
with open("src/chatwidget/slash_dispatch.rs", "r") as f:
    data = f.read()

data = data.replace("self.add_history_message", "self.add_error_message")
data = data.replace(
    "SlashCommand::TestApproval => {",
    "SlashCommand::BgDream | SlashCommand::Help => {},\n            SlashCommand::TestApproval => {"
)

with open("src/chatwidget/slash_dispatch.rs", "w") as f:
    f.write(data)

# 3. chatwidget.rs
with open("src/chatwidget.rs", "r") as f:
    data = f.read()

data = data.replace(
    "                id: \"\".to_string(),\n            }]",
    "                id: \"\".to_string(),\n                additional_speed_tiers: Vec::new(),\n            }]"
)

with open("src/chatwidget.rs", "w") as f:
    f.write(data)

# 4. status/card.rs
with open("src/status/card.rs", "r") as f:
    data = f.read()

data = data.replace(
    "let agents_summary = compose_agents_summary(config);",
    "let agents_summary = compose_agents_summary(config, &[]);"
)

with open("src/status/card.rs", "w") as f:
    f.write(data)

# 5. slash_command.rs
with open("src/slash_command.rs", "r") as f:
    data = f.read()

data = data.replace(
    "SlashCommand::TestApproval => \"test approval request\",",
    "SlashCommand::TestApproval => \"test approval request\",\n            SlashCommand::BgDream | SlashCommand::Help => \"\","
)

data = data.replace(
    "SlashCommand::Title => false,",
    "SlashCommand::Title => false,\n            SlashCommand::BgDream | SlashCommand::Help => false,"
)

with open("src/slash_command.rs", "w") as f:
    f.write(data)

