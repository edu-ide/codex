#!/usr/bin/env bash
set -euo pipefail

TARGET_REF="${TARGET_REF:-main}"
BACKUP_BRANCH="${BACKUP_BRANCH:-tripleyoung/backup-packlimit-$(date +%Y%m%d-%H%M%S)}"

readonly PATHS_TO_STRIP=(
  "codex-rs/ilhae/target"
  "codex-rs/ilhae/test-reports"
  "codex-rs/check_output.json"
  "codex-rs/ilhae/check.json"
  "codex-rs/ilhae/err.txt"
  "codex-rs/ilhae/errors.txt"
)

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "Run this script from inside the codex git repository." >&2
  exit 1
fi

if ! command -v git-filter-repo >/dev/null 2>&1; then
  cat >&2 <<'EOF'
git-filter-repo is not installed.

Expected local install path:
  ~/.local/bin/git-filter-repo

Install example:
  python3 -m venv ~/.local/share/git-filter-repo-venv
  ~/.local/share/git-filter-repo-venv/bin/pip install git-filter-repo
  ln -sf ~/.local/share/git-filter-repo-venv/bin/git-filter-repo ~/.local/bin/git-filter-repo
EOF
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Working tree must be clean before rewriting history." >&2
  exit 1
fi

if ! git show-ref --verify --quiet "refs/heads/${TARGET_REF}"; then
  echo "Target branch '${TARGET_REF}' does not exist." >&2
  exit 1
fi

print_plan() {
  echo "This script will rewrite branch '${TARGET_REF}' and remove these paths from history:"
  for path in "${PATHS_TO_STRIP[@]}"; do
    echo "  - ${path}"
  done
  echo
  echo "Backup branch to create first: ${BACKUP_BRANCH}"
  echo
  echo "Run with:"
  echo "  $0 --apply"
}

if [[ "${1:-}" != "--apply" ]]; then
  print_plan
  exit 0
fi

git branch "${BACKUP_BRANCH}" "${TARGET_REF}"

filter_args=(
  --force
  --refs "refs/heads/${TARGET_REF}"
)

for path in "${PATHS_TO_STRIP[@]}"; do
  filter_args+=(--path "${path}")
done

filter_args+=(--invert-paths)

git-filter-repo "${filter_args[@]}"

cat <<EOF
Rewrite complete.

Backup branch:
  ${BACKUP_BRANCH}

Suggested follow-up:
  git log --stat ${TARGET_REF}..${BACKUP_BRANCH}
  git push --force-with-lease origin ${TARGET_REF}
EOF
