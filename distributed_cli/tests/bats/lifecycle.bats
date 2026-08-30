#!/usr/bin/env bats

setup_file() {
  : "${DISTRIBUTED_BIN:?set DISTRIBUTED_BIN to the compiled distributed binary}"
  [ -x "$DISTRIBUTED_BIN" ]
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

@test "dev reports usable processes, rebuilds selectively, and cleans descendants" {
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
tail -f /dev/null &
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
    "shutdown_ms": 1000,
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
