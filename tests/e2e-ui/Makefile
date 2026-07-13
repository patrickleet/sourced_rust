# Fieldnote e2e-ui template — run from this directory.
#
#   make up          # Docker: Postgres + Zitadel + bootstrap env
#   make run         # API + UI (uses e2e-ui.env if present)
#   make test        # offline unit/suite/UI structural
#   make test-live   # against stack (E2E_STACK=1)

.PHONY: all up down run run-api stop test test-domain test-suite test-live \
	ui-install ui-build ui-test check clean help

BIND         ?= 127.0.0.1:8791
API_PORT     := $(shell echo $(BIND) | sed 's/.*://')
BASE_URL     ?= http://$(BIND)
UI_PORT      ?= 5180
UI_URL       ?= http://127.0.0.1:$(UI_PORT)
ENV_FILE     ?= e2e-ui.env
NPM          ?= npm
CARGO_TEST_FLAGS ?= -- --nocapture

# Prefer env file from make up
ifneq (,$(wildcard $(ENV_FILE)))
  include $(ENV_FILE)
  export
endif

DATABASE_URL ?= sqlite:./e2e-ui.db?mode=rwc

all: run

## Docker stack + OIDC bootstrap → e2e-ui.env
up:
	chmod +x scripts/up.sh
	./scripts/up.sh

down:
	docker compose -f docker/docker-compose.yml down

## API + UI (loads e2e-ui.env when present)
run: ui-install
	@set -e; \
	if [ -f $(ENV_FILE) ]; then set -a; . ./$(ENV_FILE); set +a; fi; \
	lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.pid .make-ui.pid .make-runner.log; \
	echo "starting API ($(DATABASE_URL)) on $(BASE_URL) …"; \
	cargo run -p e2e-runner > .make-runner.log 2>&1 & \
	echo $$! > .make-runner.pid; \
	ok=0; \
	for i in $$(seq 1 80); do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' -X POST '$(BASE_URL)/graphql' \
			-H 'content-type: application/json' \
			-d '{"query":"{ __typename }"}' 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ] || [ "$$code" = "401" ]; then ok=1; break; fi; \
		sleep 0.25; \
	done; \
	if [ "$$ok" != "1" ]; then \
		echo "API failed:"; tail -80 .make-runner.log; exit 1; \
	fi; \
	echo "API ready (HTTP probe $$code). starting UI …"; \
	cd ui && $(NPM) run dev -- --host 127.0.0.1 --port $(UI_PORT) & \
	echo $$! > ../.make-ui.pid; \
	cd ..; \
	cleanup() { \
		[ -f .make-ui.pid ] && kill $$(cat .make-ui.pid) 2>/dev/null || true; \
		[ -f .make-runner.pid ] && kill $$(cat .make-runner.pid) 2>/dev/null || true; \
		lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		rm -f .make-runner.pid .make-ui.pid; \
	}; \
	trap cleanup EXIT INT TERM; \
	echo ""; \
	echo "  UI     $(UI_URL)"; \
	echo "  API    $(BASE_URL)"; \
	echo "  GraphiQL $(BASE_URL)/graphql"; \
	echo "  WS     $(BASE_URL)/graphql/ws"; \
	echo "  Ctrl-C stops both"; \
	echo ""; \
	wait $$(cat .make-ui.pid) 2>/dev/null || wait

run-api:
	@if [ -f $(ENV_FILE) ]; then set -a; . ./$(ENV_FILE); set +a; fi; \
	cargo run -p e2e-runner

stop:
	@if [ -f .make-ui.pid ]; then kill $$(cat .make-ui.pid) 2>/dev/null || true; fi
	@if [ -f .make-runner.pid ]; then kill $$(cat .make-runner.pid) 2>/dev/null || true; fi
	@lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@rm -f .make-runner.pid .make-ui.pid
	@echo stopped

test: test-domain test-suite ui-install ui-build ui-test
	@echo "OK — offline domain + suite + UI build + structural tests"

test-domain:
	cargo test -p todo-domain -p chat-domain $(CARGO_TEST_FLAGS)

test-suite:
	cargo test -p e2e-suite --test behavioral $(CARGO_TEST_FLAGS)
	cargo test -p e2e-suite --test oidc_pg -- --nocapture

## Live OIDC+Postgres (requires make up + API running with e2e-ui.env)
test-live:
	@if [ ! -f $(ENV_FILE) ]; then echo "missing $(ENV_FILE) — run make up"; exit 1; fi
	set -a; . ./$(ENV_FILE); set +a; \
	E2E_STACK=1 cargo test -p e2e-suite --test oidc_pg -- --nocapture

ui-install:
	cd ui && $(NPM) install

ui-build: ui-install
	cd ui && $(NPM) run build

ui-test: ui-install
	cd ui && $(NPM) test

check:
	cargo check --workspace
	cargo test -p todo-domain -p chat-domain --no-run
	cargo test -p e2e-suite --test behavioral --no-run

clean: stop
	cargo clean
	rm -f .make-runner.log e2e-ui.db
	rm -rf ui/node_modules ui/build ui/.svelte-kit

help:
	@echo "Fieldnote e2e-ui"
	@echo "  make up         Postgres + Zitadel + OIDC bootstrap → e2e-ui.env"
	@echo "  make run        API + UI (source e2e-ui.env when present)"
	@echo "  make test       offline suite + UI structural"
	@echo "  make test-live  OIDC isolation against stack"
	@echo "  make down       docker compose down"
