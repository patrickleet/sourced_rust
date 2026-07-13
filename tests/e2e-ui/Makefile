# e2e-ui fixture — run from this directory.
#
#   make            # start API + UI (full stack)
#   make test       # run all checks
#   make help

.PHONY: all run up run-api stop test test-domain test-suite test-workspace \
	ui-install ui-build ui-test ui-test-live check fmt clean help

BIND         ?= 127.0.0.1:8791
API_PORT     := $(shell echo $(BIND) | sed 's/.*://')
BASE_URL     ?= http://$(BIND)
UI_PORT      ?= 5180
UI_URL       ?= http://127.0.0.1:$(UI_PORT)
DATABASE_URL ?= sqlite:./e2e-ui.db?mode=rwc
CARGO_TEST_FLAGS ?= -- --nocapture
NPM          ?= npm

# ── default: run the full app ───────────────────────────────────────────────

all: run

## Start API + UI together. Ctrl-C stops both.
run: up
up: ui-install
	@set -e; \
	lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.log .make-runner.pid .make-ui.log .make-ui.pid; \
	echo "starting API on $(BASE_URL) …"; \
	DATABASE_URL='$(DATABASE_URL)' BIND='$(BIND)' \
		cargo run -p e2e-runner > .make-runner.log 2>&1 & \
	echo $$! > .make-runner.pid; \
	ok=0; \
	for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 \
	         21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 \
	         41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60; do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' -X POST '$(BASE_URL)/graphql' \
			-H 'content-type: application/json' \
			-H 'x-user-id: probe' -H 'x-role: admin' \
			-d '{"query":"{ __typename }"}' 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ] || [ "$$code" = "401" ]; then ok=1; break; fi; \
		sleep 0.25; \
	done; \
	if [ "$$ok" != "1" ]; then \
		echo "API failed to start; log:"; \
		tail -80 .make-runner.log || true; \
		kill $$(cat .make-runner.pid) 2>/dev/null || true; \
		rm -f .make-runner.pid; \
		exit 1; \
	fi; \
	echo "API ready."; \
	echo "starting UI on $(UI_URL) (proxies /graphql[ws], /todo.*, /chat.* → API) …"; \
	cd ui && E2E_BASE_URL='$(BASE_URL)' $(NPM) run dev -- --host 127.0.0.1 --port $(UI_PORT) & \
	echo $$! > ../.make-ui.pid; \
	cd ..; \
	cleanup() { \
		echo ""; \
		echo "stopping …"; \
		if [ -f .make-ui.pid ]; then kill $$(cat .make-ui.pid) 2>/dev/null || true; fi; \
		if [ -f .make-runner.pid ]; then kill $$(cat .make-runner.pid) 2>/dev/null || true; fi; \
		lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		rm -f .make-runner.pid .make-ui.pid; \
	}; \
	trap cleanup EXIT INT TERM; \
	echo ""; \
	echo "┌─────────────────────────────────────────────┐"; \
	echo "│  UI     $(UI_URL)  ( /  todos, /chat ) │"; \
	echo "│  API    $(BASE_URL)                   │"; \
	echo "│  GraphiQL $(BASE_URL)/graphql         │"; \
	echo "│  WS sub $(BASE_URL)/graphql/ws        │"; \
	echo "│  Ctrl-C stops both                             │"; \
	echo "└─────────────────────────────────────────────┘"; \
	echo ""; \
	wait $$(cat .make-ui.pid) 2>/dev/null || wait

## API only (no UI)
run-api:
	DATABASE_URL='$(DATABASE_URL)' BIND='$(BIND)' cargo run -p e2e-runner

## Kill leftover API/UI processes from a previous make run
stop:
	@if [ -f .make-ui.pid ]; then kill $$(cat .make-ui.pid) 2>/dev/null || true; fi
	@if [ -f .make-runner.pid ]; then kill $$(cat .make-runner.pid) 2>/dev/null || true; fi
	@lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@rm -f .make-runner.pid .make-ui.pid
	@echo "stopped"

# ── test everything ─────────────────────────────────────────────────────────

test: test-domain test-suite ui-install ui-build ui-test ui-test-live
	@echo ""
	@echo "OK — domain + suite + UI build + offline UI + live UI contract"

test-domain:
	cargo test -p todo-domain -p chat-domain $(CARGO_TEST_FLAGS)

test-suite:
	cargo test -p e2e-suite --test behavioral $(CARGO_TEST_FLAGS)

test-workspace:
	cargo test --workspace $(CARGO_TEST_FLAGS)

check:
	cargo check --workspace
	cargo test -p todo-domain -p chat-domain --no-run
	cargo test -p e2e-suite --test behavioral --no-run

fmt:
	cargo fmt --all

# ── UI ──────────────────────────────────────────────────────────────────────

ui-install:
	cd ui && $(NPM) install

ui-build: ui-install
	cd ui && $(NPM) run build

ui-test: ui-install
	cd ui && $(NPM) test

ui-test-live: ui-install
	@set -e; \
	lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.log .make-ui.db; \
	DATABASE_URL='sqlite:./.make-ui.db?mode=rwc' BIND='$(BIND)' \
		cargo run -p e2e-runner > .make-runner.log 2>&1 & \
	echo $$! > .make-runner.pid; \
	trap 'kill $$(cat .make-runner.pid) 2>/dev/null || true; rm -f .make-runner.pid' EXIT INT TERM; \
	echo "waiting for $(BASE_URL) …"; \
	ok=0; \
	for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 \
	         21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40; do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' -X POST '$(BASE_URL)/graphql' \
			-H 'content-type: application/json' \
			-H 'x-user-id: probe' -H 'x-role: admin' \
			-d '{"query":"{ __typename }"}' 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ] || [ "$$code" = "401" ]; then ok=1; break; fi; \
		sleep 0.25; \
	done; \
	if [ "$$ok" != "1" ]; then \
		echo "runner did not become ready; log:"; \
		tail -50 .make-runner.log || true; \
		exit 1; \
	fi; \
	E2E_BASE_URL='$(BASE_URL)' $(NPM) --prefix ui test; \
	echo "ui-test-live OK"

clean: stop
	cargo clean
	rm -f .make-runner.log .make-ui.db e2e-ui.db
	rm -rf ui/node_modules ui/build ui/.svelte-kit

help:
	@echo "e2e-ui Makefile"
	@echo ""
	@echo "  make / make run    start API + UI (Ctrl-C stops both)"
	@echo "  make run-api       API only"
	@echo "  make stop          kill leftover API/UI from make run"
	@echo "  make test          domain + suite + UI build + UI contracts"
	@echo "  make test-domain   aggregate unit tests"
	@echo "  make test-suite    behavioral T1–T6"
	@echo "  make ui-build      npm install + production build"
	@echo "  make clean         stop + cargo clean + UI artifacts"
	@echo ""
	@echo "After make run:"
	@echo "  UI        $(UI_URL)"
	@echo "  API       $(BASE_URL)"
	@echo "  GraphiQL  $(BASE_URL)/graphql"
