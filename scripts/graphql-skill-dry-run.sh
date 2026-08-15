#!/usr/bin/env bash
# Dry-run: agent following only README + distributed-graphql skill can
# scaffold --query-api and add a model exposure. Captures evidence for
# tasks/graphql-qs-13-docs-skills AC2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRATCH="${GROK_SCRATCH:-${1:-}}"
if [[ -z "$SCRATCH" ]]; then
  echo "usage: GROK_SCRATCH=<dir> $0   OR   $0 <scratch-dir>" >&2
  exit 2
fi
mkdir -p "$SCRATCH"
LOG="$SCRATCH/skill-dry-run.log"
exec > >(tee "$LOG") 2>&1

echo "=== skill dry-run start $(date -u +%Y-%m-%dT%H:%M:%SZ) ==="
echo "ROOT=$ROOT"
echo "SCRATCH=$SCRATCH"

# 1) Load only README + skill (existence + frontmatter — the agent would read these)
test -f "$ROOT/README.md"
test -f "$ROOT/docs/graphql.md"
SKILL="$ROOT/distributed_cli/skills/distributed-graphql/SKILL.md"
test -f "$SKILL"
grep -q '^name: distributed-graphql' "$SKILL"
grep -q 'src/query/' "$SKILL"
grep -q 'with_graphql' "$SKILL"
echo "OK: README + skill present and teach query layout"

# 2) Build distributed
cd "$ROOT"
cargo build -p distributed_cli --quiet
DISTRIBUTED="$ROOT/target/debug/distributed"
test -x "$DISTRIBUTED"

# 3) Scaffold --query-api (as skill documents)
DEMO="$SCRATCH/skill-dry-run-service"
rm -rf "$DEMO"
"$DISTRIBUTED" scaffold skill-dry-run \
  --path "$DEMO" \
  --query-api \
  --store sqlite \
  --model Order \
  --distributed-path "$ROOT" \
  --force
echo "OK: scaffold --query-api wrote $DEMO"

# 4) Pin distributed path to this crate (scratch dirs break relative paths)
python3 -c "
from pathlib import Path
import re
p = Path(r'''$DEMO/Cargo.toml''')
t = p.read_text()
root = Path(r'''$ROOT''').resolve().as_posix()
t = re.sub(
    r'distributed = \{ path = \"[^\"]+\"',
    'distributed = { path = \"' + root + '\"',
    t,
    count=1,
)
p.write_text(t)
print('fixed distributed path ->', root)
"

# 5) Agent exercise: model exposure files exist; annotate following skill
test -f "$DEMO/src/query/order.rs"
test -f "$DEMO/src/query/roles.rs"
grep -q 'pub const USER' "$DEMO/src/query/roles.rs"
grep -q 'fn permissions' "$DEMO/src/query/order.rs"
echo '// skill dry-run: role USER granted via grant_all in mod.rs; tighten in permissions()' >> "$DEMO/src/query/order.rs"
echo "OK: model exposure files present (order + roles)"

# 6) cargo check (must compile against real distributed)
cd "$DEMO"
cargo check
echo "OK: cargo check exit=$?"

echo "=== skill dry-run SUCCESS ==="
