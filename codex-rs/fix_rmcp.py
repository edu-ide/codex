import os
import re

for root, _, files in os.walk("."):
    for file in files:
        if file == "Cargo.toml":
            path = os.path.join(root, file)
            with open(path, "r") as f:
                content = f.read()
            
            new_content = re.sub(
                r'rmcp\s*=\s*\{\s*version\s*=\s*"0\.15\.0"',
                'rmcp = { workspace = true',
                content
            )
            
            if new_content != content:
                print(f"Updated rmcp version in {path}")
                with open(path, "w") as f:
                    f.write(new_content)
