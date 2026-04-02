import sys

mod_path = '/mnt/nvme0n1p2/workspace/monorepo/services/ilhae-agent/ilhae-proxy/src/context_proxy/mod.rs'
with open(mod_path, 'r', encoding='utf-8') as f:
    lines = f.readlines()

# delete 64-77
del lines[63:77]

with open(mod_path, 'w', encoding='utf-8') as f:
    f.writelines(lines)
