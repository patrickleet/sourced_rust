# Fieldnote e2e-ui template — run from this directory.
#
#   make up          # Docker: Postgres + Zitadel + bootstrap env
#   make run         # API + UI (uses e2e-ui.env if present)
#   make test        # offline unit/suite/UI structural
#   make test-live   # against stack (E2E_STACK=1)

.PHONY: all up down run run-api stop test test-domain test-suite test-live \
	ui-install ui-build ui-test export-sdl gen-gql check clean help

# Defaults only — do NOT `include e2e-ui.env` (shell-quoted dotenv breaks Make).
# Recipes `source` the env file so values stay clean.
BIND         ?= 127.0.0.1:8791
API_PORT     ?= 8791
BASE_URL     ?= http://127.0.0.1:8791
UI_PORT      ?= 5180
UI_URL       ?= http://127.0.0.1:5180
ENV_FILE     ?= e2e-ui.env
NPM          ?= npm
CARGO_TEST_FLAGS ?= -- --nocapture

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
	_bind="$${BIND:-127.0.0.1:8791}"; \
	_api_port="$${_bind##*:}"; \
	_base="$${E2E_API_ORIGIN:-http://$${_bind}}"; \
	_ui_port="$(UI_PORT)"; \
	_ui="$${E2E_UI_ORIGIN:-http://127.0.0.1:$${_ui_port}}"; \
	lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.pid .make-ui.pid .make-runner.log; \
	echo "starting API ($${DATABASE_URL:-sqlite}) on $$_base …"; \
	cargo run -p e2e-runner > .make-runner.log 2>&1 & \
	echo $$! > .make-runner.pid; \
	ok=0; \
	for i in $$(seq 1 80); do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' -X POST "$${_base}/graphql" \
			-H 'content-type: application/json' \
			-d '{"query":"{ __typename }"}' 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ] || [ "$$code" = "401" ]; then ok=1; break; fi; \
		sleep 0.25; \
	done; \
	if [ "$$ok" != "1" ]; then \
		echo "API failed:"; tail -80 .make-runner.log; exit 1; \
	fi; \
	echo "API ready (HTTP probe $$code). starting UI …"; \
	cd ui && $(NPM) run dev -- --host 127.0.0.1 --port $$_ui_port & \
	echo $$! > ../.make-ui.pid; \
	cd ..; \
	cleanup() { \
		[ -f .make-ui.pid ] && kill $$(cat .make-ui.pid) 2>/dev/null || true; \
		[ -f .make-runner.pid ] && kill $$(cat .make-runner.pid) 2>/dev/null || true; \
		lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		rm -f .make-runner.pid .make-ui.pid; \
	}; \
	trap cleanup EXIT INT TERM; \
	echo ""; \
	echo "  UI     $$_ui"; \
	echo "  API    $$_base"; \
	echo "  GraphiQL $$_base/graphql"; \
	echo "  WS     $$_base/graphql/ws"; \
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

## Role SDL for UI codegen (GraphqlEngine::sdl_for_role → ui/schema/user.graphql)
export-sdl:
	cargo run -q -p e2e-runner --bin e2e-export-sdl -- ui/schema/user.graphql

## Export role SDL + regenerate TypedDocumentNode from co-located *.gql
gen-gql: export-sdl ui-install
	cd ui && $(NPM) run gen:gql

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
	@echo "  make export-sdl role user SDL → ui/schema/user.graphql (Rust engine)"
	@echo "  make gen-gql    export-sdl + ui npm run gen:gql"
	@echo "  make down       docker compose down"
