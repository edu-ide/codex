import re

with open("src/app_server_session.rs", "r") as f:
    data = f.read()

# Add the fields to Turn structs
data = re.sub(
    r"(Turn\s*\{[\s\S]*?)(error:\s*(?:None|Some\([\s\S]*?\}\)),\n)",
    r"\1\2                        started_at: None,\n                        completed_at: None,\n                        duration_ms: None,\n",
    data
)

with open("src/app_server_session.rs", "w") as f:
    f.write(data)
