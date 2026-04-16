import re

def fix_file(filename):
    with open(filename, "r") as f:
        data = f.read()

    # ModelPreset
    data = re.sub(
        r"(ModelPreset\s*\{[^\}]*?max_tokens:[^\}]*?)(\n\s*\})",
        r"\1,\n                additional_speed_tiers: Vec::new()\2",
        data, flags=re.DOTALL
    )

    # Turn
    data = re.sub(
        r"(Turn\s*\{[^\}]*?message:[^\}]*?)(\n\s*\})",
        r"\1,\n                started_at: None, completed_at: None, duration_ms: None\2",
        data, flags=re.DOTALL
    )

    with open(filename, "w") as f:
        f.write(data)

fix_file("src/app_server_session.rs")
fix_file("src/chatwidget.rs")

