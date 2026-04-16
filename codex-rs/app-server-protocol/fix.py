import re

with open("src/protocol/v2.rs", "r") as f:
    data = f.read()

data = data.replace(
    "            _meta: value.meta,\n        }\n    }\n}",
    "            meta: value.meta,\n        }\n    }\n}"
)

with open("src/protocol/v2.rs", "w") as f:
    f.write(data)
