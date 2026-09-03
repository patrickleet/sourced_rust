#!/usr/bin/env bats

setup_file() {
  : "${DISTRIBUTED_BIN:?set DISTRIBUTED_BIN to the compiled distributed binary}"
  [ -x "$DISTRIBUTED_BIN" ]
  DISTRIBUTED_ROOT="$(cd "$(dirname "$DISTRIBUTED_BIN")/../.." && pwd -P)"
  export DISTRIBUTED_ROOT
}

setup() {
  ROOT="$(mktemp -d "$BATS_TEST_TMPDIR/distributed-lifecycle.XXXXXX")"
  ROOT="$(cd "$ROOT" && pwd -P)"
  DEV_LOG="$ROOT/dev.log"
  SUPERVISOR_PID=""
  write_fixture
}

teardown() {
  if [ -n "${SUPERVISOR_PID:-}" ] && kill -0 "$SUPERVISOR_PID" 2>/dev/null; then
    kill -INT "$SUPERVISOR_PID" 2>/dev/null || true
    wait_for_exit "$SUPERVISOR_PID" 100 || kill -TERM "$SUPERVISOR_PID" 2>/dev/null || true
    wait "$SUPERVISOR_PID" 2>/dev/null || true
  fi
  if [ -f "$ROOT/dev-descendants.log" ]; then
    while IFS= read -r pid; do
      kill -TERM "$pid" 2>/dev/null || true
    done < "$ROOT/dev-descendants.log"
  fi
}

@test "project build and dev are zero-config and invalid typed exports fail before UI" {
  PROJECT="$ROOT/zero-config-app"
  run "$DISTRIBUTED_BIN" scaffold zero-config-app \
    --path "$PROJECT" \
    --distributed-path "$DISTRIBUTED_ROOT" \
    --query-api \
    --store sqlite
  [ "$status" -eq 0 ]

  mkdir -p "$PROJECT/ui" "$ROOT/bin"
  printf '{"name":"zero-config-ui","private":true,"scripts":{"build":"vite build"},"dependencies":{"@hops-ops/distributed":"file:%s/js"}}\n' \
    "$DISTRIBUTED_ROOT" \
    > "$PROJECT/ui/package.json"
  ZERO_CONFIG_REAL_NPM="$(command -v npm)"
  [ -n "$ZERO_CONFIG_REAL_NPM" ]
  cat > "$ROOT/bin/npm" <<'SCRIPT'
#!/bin/sh
set -eu
if [ -f package.json ] && grep -Fq '"name": "@hops-ops/distributed"' package.json; then
  exec "$ZERO_CONFIG_REAL_NPM" "$@"
fi
printf '%s:%s\n' "$PWD" "$*" > "$ZERO_CONFIG_NPM_LOG"
if [ "$1" = "run" ] && [ "$2" = "dev" ]; then
  exec python3 -m http.server "${UI_PORT:-15180}" --bind 127.0.0.1
fi
mkdir -p node_modules/@hops-ops
[ -e node_modules/@hops-ops/distributed ] || \
  ln -s "$DISTRIBUTED_ROOT/js" node_modules/@hops-ops/distributed
mkdir -p .svelte-kit/output
SCRIPT
  chmod +x "$ROOT/bin/npm"

  run env \
    PATH="$ROOT/bin:$PATH" \
    ZERO_CONFIG_NPM_LOG="$ROOT/npm.log" \
    ZERO_CONFIG_REAL_NPM="$ZERO_CONFIG_REAL_NPM" \
    "$DISTRIBUTED_BIN" build "$PROJECT" --output json
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ok":true'* ]]
  [ -x "$PROJECT/target/debug/zero-config-app" ]
  [ -f "$ROOT/npm.log" ]
  grep -F "$PROJECT/ui:run build" "$ROOT/npm.log"
  [ -f "$DISTRIBUTED_ROOT/js/dist/index.js" ]
  [ "$(find "$PROJECT/.distributed/javascript" -name '*.json' -type f | wc -l | tr -d ' ')" -eq 1 ]
  [ ! -e "$PROJECT/distributed.contracts.json" ]
  [ ! -e "$PROJECT/distributed.lifecycle.json" ]
  manifest_count="$(find "$PROJECT/.distributed/lifecycle/generations" \
    -path '*/artifacts/application-manifest.json' -type f | wc -l | tr -d ' ')"
  [ "$manifest_count" -eq 1 ]

  API_PORT="$(free_port)"
  UI_PORT_VALUE="$(free_port)"
  while [ "$UI_PORT_VALUE" = "$API_PORT" ]; do
    UI_PORT_VALUE="$(free_port)"
  done
  env \
    PATH="$ROOT/bin:$PATH" \
    ZERO_CONFIG_NPM_LOG="$ROOT/npm.log" \
    ZERO_CONFIG_REAL_NPM="$ZERO_CONFIG_REAL_NPM" \
    BIND="127.0.0.1:$API_PORT" \
    UI_HOST="127.0.0.1" \
    UI_PORT="$UI_PORT_VALUE" \
    DISTRIBUTED_GRAPHQL_PROTOCOL_TOKEN_KEY="0123456789abcdef0123456789abcdef" \
    "$DISTRIBUTED_BIN" dev "$PROJECT" > "$DEV_LOG" 2>&1 &
  SUPERVISOR_PID=$!
  wait_for_log 'lifecycle dev: ready generation=' 300
  grep -F "lifecycle dev: process api ready http://127.0.0.1:$API_PORT" "$DEV_LOG"
  grep -F "lifecycle dev: process ui ready http://127.0.0.1:$UI_PORT_VALUE" "$DEV_LOG"

  kill -INT "$SUPERVISOR_PID"
  wait_for_exit "$SUPERVISOR_PID" 200
  wait "$SUPERVISOR_PID"
  dev_status=$?
  SUPERVISOR_PID=""
  [ "$dev_status" -eq 0 ]

  # A conventionally named service is only a candidate until its typed export
  # compiles. Reject an older/incompatible checkout before rebuilding the UI.
  active_before="$(cat "$PROJECT/.distributed/lifecycle/active.json")"
  sed 's/application_manifest, //' "$PROJECT/src/lib.rs" > "$PROJECT/src/lib.rs.next"
  mv "$PROJECT/src/lib.rs.next" "$PROJECT/src/lib.rs"
  printf 'npm-must-not-run\n' > "$ROOT/npm.log"

  run env \
    PATH="$ROOT/bin:$PATH" \
    ZERO_CONFIG_NPM_LOG="$ROOT/npm.log" \
    ZERO_CONFIG_REAL_NPM="$ZERO_CONFIG_REAL_NPM" \
    "$DISTRIBUTED_BIN" build "$PROJECT"
  [ "$status" -ne 0 ]
  [[ "$output" == *'introspecting typed application zero-config-app'* ]]
  [[ "$output" == *'must publicly export a zero-argument function returning `distributed::ApplicationManifest`'* ]]
  [[ "$output" != *'compiling SvelteKit UI'* ]]
  [ "$(cat "$ROOT/npm.log")" = 'npm-must-not-run' ]
  [ "$(cat "$PROJECT/.distributed/lifecycle/active.json")" = "$active_before" ]
}

@test "linked checkout skew is structured and fails before build or dev startup" {
  PROJECT="$ROOT/skewed-app"
  run "$DISTRIBUTED_BIN" scaffold skewed-app \
    --path "$PROJECT" \
    --distributed-path "$DISTRIBUTED_ROOT" \
    --query-api \
    --store sqlite
  [ "$status" -eq 0 ]

  OTHER_JS="$ROOT/other-distributed/js"
  mkdir -p "$PROJECT/ui" "$OTHER_JS"
  printf '{"name":"@hops-ops/distributed","version":"0.1.0","scripts":{"build":"tsc"},"exports":{".":"./dist/index.js"}}\n' \
    > "$OTHER_JS/package.json"
  printf '{"name":"skewed-ui","private":true,"dependencies":{"@hops-ops/distributed":"file:%s"}}\n' \
    "$OTHER_JS" \
    > "$PROJECT/ui/package.json"

  run "$DISTRIBUTED_BIN" build "$PROJECT" --output json
  [ "$status" -eq 1 ]
  [[ "$output" == *'"code":"CTL-FRAMEWORK-IDENTITY-MISMATCH"'* ]]
  [[ "$output" == *'"affected_components":["rust","cli","javascript"]'* ]]
  [[ "$output" == *'"expected"'* ]]
  [[ "$output" == *'"repair"'* ]]
  [ ! -e "$PROJECT/target/debug/skewed-app" ]
  [ ! -e "$PROJECT/ui/node_modules" ]
  [ ! -e "$PROJECT/.distributed/lifecycle/active.json" ]

  run "$DISTRIBUTED_BIN" dev "$PROJECT"
  [ "$status" -eq 1 ]
  [[ "$output" == *'incompatible Distributed framework members'* ]]
  [ ! -e "$PROJECT/target/debug/skewed-app" ]
  [ ! -e "$PROJECT/ui/node_modules" ]
  [ ! -e "$PROJECT/.distributed/lifecycle/active.json" ]
}

@test "build activates atomically and check reports drift without replacing active" {
  run "$DISTRIBUTED_BIN" build --root "$ROOT" --output json
  [ "$status" -eq 0 ]
  [[ "$output" == *'"ok":true'* ]]
  [[ "$output" == *'"executed":["application","plan"]'* ]]
  [ -f "$ROOT/dist/distributed/active.json" ]
  active_before="$(cat "$ROOT/dist/distributed/active.json")"

  run "$DISTRIBUTED_BIN" build --root "$ROOT" --check --output json
  [ "$status" -eq 0 ]
  [[ "$output" == *'"drift":[]'* ]]
  [ "$(cat "$ROOT/dist/distributed/active.json")" = "$active_before" ]

  printf 'second\n' > "$ROOT/src/input.txt"
  run "$DISTRIBUTED_BIN" build --root "$ROOT" --check --output json
  [ "$status" -eq 1 ]
  [[ "$output" == *'"ok":false'* ]]
  [[ "$output" == *'"node_id":"application"'* ]]
  [ "$(cat "$ROOT/dist/distributed/active.json")" = "$active_before" ]

  : > "$ROOT/fail-plan"
  run "$DISTRIBUTED_BIN" build --root "$ROOT" --output json
  [ "$status" -ne 0 ]
  [[ "$output" == *'injected plan failure'* ]]
  [ "$(cat "$ROOT/dist/distributed/active.json")" = "$active_before" ]
}

@test "dev reports process readiness, rebuilds selectively, and cleans descendants" {
  "$DISTRIBUTED_BIN" dev --root "$ROOT" > "$DEV_LOG" 2>&1 &
  SUPERVISOR_PID=$!

  wait_for_log 'lifecycle dev: ready generation=' 300
  grep -F 'lifecycle dev: process api ready http://127.0.0.1:18081/health' "$DEV_LOG"
  grep -F 'lifecycle dev: process ui ready http://127.0.0.1:15181' "$DEV_LOG"
  grep -F "api:api:$ROOT/src" "$ROOT/dev-environment.log"
  grep -F "ui:ui:$ROOT/src" "$ROOT/dev-environment.log"

  printf 'second\n' > "$ROOT/src/input.txt"
  wait_for_log 'invalidated=application,plan restarted=api' 300
  [ "$(grep -c '^api:' "$ROOT/dev-process.log")" -eq 2 ]
  [ "$(grep -c '^ui:' "$ROOT/dev-process.log")" -eq 1 ]

  kill -INT "$SUPERVISOR_PID"
  wait_for_exit "$SUPERVISOR_PID" 200
  wait "$SUPERVISOR_PID"
  dev_status=$?
  SUPERVISOR_PID=""
  [ "$dev_status" -eq 0 ]
  grep -F 'process=api restarts=1' "$DEV_LOG"
  grep -F 'process=ui restarts=0' "$DEV_LOG"

  while IFS= read -r pid; do
    ! kill -0 "$pid" 2>/dev/null
  done < "$ROOT/dev-descendants.log"
}

@test "Ctrl-C cancels the initial build before any process starts" {
  : > "$ROOT/slow-plan"
  "$DISTRIBUTED_BIN" dev --root "$ROOT" > "$DEV_LOG" 2>&1 &
  SUPERVISOR_PID=$!
  wait_for_file "$ROOT/build-plan-starts.log" 300

  kill -INT "$SUPERVISOR_PID"
  wait_for_exit "$SUPERVISOR_PID" 200
  wait "$SUPERVISOR_PID" || dev_status=$?
  dev_status="${dev_status:-0}"
  SUPERVISOR_PID=""

  [ "$dev_status" -ne 0 ]
  grep -F 'lifecycle build was canceled' "$DEV_LOG"
  [ ! -e "$ROOT/dev-process.log" ]
}

free_port() {
  python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
}

wait_for_log() {
  needle=$1
  attempts=$2
  i=0
  while [ "$i" -lt "$attempts" ]; do
    if [ -f "$DEV_LOG" ] && grep -Fq "$needle" "$DEV_LOG"; then
      return 0
    fi
    if [ -n "${SUPERVISOR_PID:-}" ] && ! kill -0 "$SUPERVISOR_PID" 2>/dev/null; then
      cat "$DEV_LOG" >&3
      return 1
    fi
    sleep 0.05
    i=$((i + 1))
  done
  cat "$DEV_LOG" >&3
  return 1
}

wait_for_file() {
  path=$1
  attempts=$2
  i=0
  while [ "$i" -lt "$attempts" ]; do
    [ -f "$path" ] && return 0
    if [ -n "${SUPERVISOR_PID:-}" ] && ! kill -0 "$SUPERVISOR_PID" 2>/dev/null; then
      cat "$DEV_LOG" >&3
      return 1
    fi
    sleep 0.05
    i=$((i + 1))
  done
  cat "$DEV_LOG" >&3
  return 1
}

wait_for_exit() {
  pid=$1
  attempts=$2
  i=0
  while [ "$i" -lt "$attempts" ]; do
    ! kill -0 "$pid" 2>/dev/null && return 0
    sleep 0.05
    i=$((i + 1))
  done
  return 1
}

write_fixture() {
  mkdir -p "$ROOT/src" "$ROOT/plan" "$ROOT/generated"
  printf 'first\n' > "$ROOT/src/input.txt"
  printf 'local\n' > "$ROOT/plan/input.txt"
  printf 'application:first\n' > "$ROOT/generated/application.json"
  printf 'plan:local:application:first\n' > "$ROOT/generated/plan.json"

  cat > "$ROOT/build-app.sh" <<'SCRIPT'
#!/bin/sh
set -eu
root=$1
sed 's/^/application:/' "$root/src/input.txt"
SCRIPT

  cat > "$ROOT/build-plan.sh" <<'SCRIPT'
#!/bin/sh
set -eu
root=$1
stage=$2
if [ -f "$root/fail-plan" ]; then
  printf 'injected plan failure\n' >&2
  exit 17
fi
if [ -f "$root/slow-plan" ]; then
  printf 'started\n' >> "$root/build-plan-starts.log"
  while [ -f "$root/slow-plan" ]; do sleep 0.05; done
fi
printf 'plan:' > "$stage/generated/plan.json"
tr -d '\n' < "$root/plan/input.txt" >> "$stage/generated/plan.json"
printf ':' >> "$stage/generated/plan.json"
cat "$stage/generated/application.json" >> "$stage/generated/plan.json"
SCRIPT

  cat > "$ROOT/dev-child.sh" <<'SCRIPT'
#!/bin/sh
set -eu
root=$1
name=$2
printf '%s:%s:%s\n' "$name" "$DISTRIBUTED_GENERATION_ID" "$DISTRIBUTED_RELEASE_ID" >> "$root/dev-process.log"
printf '%s:%s:%s\n' "$name" "$DEV_FIXTURE_NAME" "$PWD" >> "$root/dev-environment.log"
trap 'exit 0' TERM
/bin/sh -c 'trap "" TERM; while :; do sleep 1; done' &
descendant=$!
printf '%s\n' "$descendant" >> "$root/dev-descendants.log"
wait "$descendant"
SCRIPT
  chmod +x "$ROOT/build-app.sh" "$ROOT/build-plan.sh" "$ROOT/dev-child.sh"

  cat > "$ROOT/distributed.contracts.json" <<'JSON'
{
  "schema_version": 1,
  "entries": {
    "application": {
      "id": "application",
      "kind": "application_manifest",
      "scope": { "id": "application/bats" },
      "owner": "application/bats",
      "identity": { "kind": "application_manifest", "value": "ref:application" },
      "provenance": { "sources": ["src/input.txt"], "generator": "bats.application" },
      "outputs": { "manifest": "generated/application.json" },
      "lifecycle": ["build", "check", "dev"]
    },
    "plan": {
      "id": "plan",
      "kind": "deployment_plan",
      "scope": { "id": "deployment/bats" },
      "owner": "deployment/bats",
      "identity": { "kind": "deployment_plan", "value": "ref:plan" },
      "provenance": {
        "sources": ["generated/application.json", "plan/input.txt"],
        "generator": "bats.plan"
      },
      "predecessor": {
        "entry_id": "application",
        "identity": { "kind": "application_manifest", "value": "ref:application" }
      },
      "outputs": { "plan": "generated/plan.json" },
      "lifecycle": ["build", "check", "dev"]
    }
  }
}
JSON

  cat > "$ROOT/distributed.lifecycle.json" <<'JSON'
{
  "schema_version": 1,
  "application": "bats",
  "source": {
    "rust": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "cli": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "javascript": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  },
  "roots": ["plan"],
  "executors": {
    "bats.application": {
      "identity": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
      "program": "/bin/sh",
      "args": ["{root}/build-app.sh", "{root}"],
      "stdout": "generated/application.json"
    },
    "bats.plan": {
      "identity": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
      "program": "/bin/sh",
      "args": ["{root}/build-plan.sh", "{root}", "{stage}"]
    }
  },
  "dev": {
    "poll_ms": 20,
    "debounce_ms": 30,
    "shutdown_ms": 100,
    "processes": {
      "api": {
        "program": "/bin/sh",
        "args": ["{root}/dev-child.sh", "{root}", "{process}"],
        "cwd": "src",
        "env": { "DEV_FIXTURE_NAME": "{process}" },
        "url": "http://127.0.0.1:18081/health",
        "restart_on": ["application"],
        "ready": {
          "program": "/bin/test",
          "args": ["-f", "{root}/dist/distributed/active.json"],
          "interval_ms": 10,
          "timeout_ms": 2000
        }
      },
      "ui": {
        "program": "/bin/sh",
        "args": ["{root}/dev-child.sh", "{root}", "{process}"],
        "cwd": "src",
        "env": { "DEV_FIXTURE_NAME": "{process}" },
        "url": "http://127.0.0.1:15181",
        "restart_on": [],
        "ready_after_ms": 20
      }
    }
  }
}
JSON
}
