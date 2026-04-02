import json
with open("check.json") as f:
    for line in f:
        try:
            msg = json.loads(line)
            if "reason" in msg and msg["reason"].get("level") == "error":
                print(msg["reason"]["message"])
                for span in msg["reason"].get("spans", []):
                    if span["is_primary"]:
                        print(f"  --> {span['file_name']}:{span['line_start']}:{span['column_start']}")
        except ValueError:
            pass
