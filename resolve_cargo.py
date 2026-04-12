import os

def resolve_cargo_toml(filepath):
    with open(filepath, 'r') as f:
        lines = f.readlines()
        
    resolved_lines = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith('<<<<<<< HEAD'):
            head_lines = []
            i += 1
            while i < len(lines) and not lines[i].startswith('======='):
                head_lines.append(lines[i])
                i += 1
            
            upstream_lines = []
            i += 1
            while i < len(lines) and not lines[i].startswith('>>>>>>>'):
                upstream_lines.append(lines[i])
                i += 1
            
            # Combine logic
            head_str = "".join(head_lines)
            up_str = "".join(upstream_lines)
            
            # If it's just [package.version] being removed upstream, keep HEAD's version if needed, 
            # or just take HEAD without version if upstream removed it. Actually, upstream removes version from crates 
            # if they are handled by workspace. Since we are moving to workspace, let's keep upstream's version removal.
            if '[package.version]' in head_str and '[package.version]' not in up_str:
                # upstream removed it, use upstream
                resolved_lines.extend(upstream_lines)
            elif '[package.version]' in up_str and '[package.version]' not in head_str:
                resolved_lines.extend(head_lines)
            else:
                # Combine both, but avoid duplicate lines
                combined = []
                for hl in head_lines:
                    if hl not in combined:
                        combined.append(hl)
                for ul in upstream_lines:
                    if ul not in combined:
                        combined.append(ul)
                resolved_lines.extend(combined)
            
        else:
            resolved_lines.append(line)
        i += 1

    with open(filepath, 'w') as f:
        f.writelines(resolved_lines)
    os.system(f"git add {filepath}")

import subprocess
out = subprocess.check_output(['git', 'diff', '--name-only', '--diff-filter=U']).decode('utf-8')
for file in out.splitlines():
    if file.endswith('Cargo.toml'):
        resolve_cargo_toml(file)
        print(f"Resolved {file}")
