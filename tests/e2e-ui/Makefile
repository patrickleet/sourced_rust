# Fieldnote e2e-ui template — run from this directory.
#
#   make up          # Docker: Postgres + Zitadel + bootstrap env
#   make run         # API + UI (uses e2e-ui.env if present)
#   make test         # offline unit/suite/UI structural
#   make test-live    # against stack (E2E_STACK=1)
#   make test-browser # Playwright UI e2e (needs make up + make run)

.PHONY: all up down run run-api stop test test-domain test-suite test-live \
	test-browser test-browser-install ui-install ui-build ui-test export-sdl gen-gql check-gql \
	export-commands gen-commands check-commands check clean help

# Defaults only — do NOT `include e2e-ui.env` (shell-quoted dotenv breaks Make).
# Recipes `source` the env file so values stay clean.
BIND         ?= 127.0.0.1:8791
API_PORT     ?= 8791
BASE_URL     ?= http://127.0.0.1:8791
UI_PORT      ?= 5180
# localhost required for Auth.js cookies over HTTP (SvelteKit Secure default)
UI_URL       ?= http://localhost:5180
UI_HOST      ?= localhost
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
	_ui_host="$(UI_HOST)"; \
	_ui="$${E2E_UI_ORIGIN:-http://$${_ui_host}:$${_ui_port}}"; \
	export AUTH_URL="$${_ui}"; \
	export AUTH_USE_SECURE_COOKIES="false"; \
	lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.pid .make-ui.pid .make-runner.log; \
	echo "starting API ($${DATABASE_URL:-sqlite}) on $$_base …"; \
	cargo run -p e2e-runner --bin e2e-ui > .make-runner.log 2>&1 & \
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
	cd ui && $(NPM) run dev -- --host $$_ui_host --port $$_ui_port & \
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

## Browser e2e (Playwright). Requires make up + make run (UI :5180, API :8791).
test-browser-install:
	$(NPM) install
	npx playwright install chromium

test-browser: test-browser-install
	@if [ ! -f $(ENV_FILE) ]; then echo "missing $(ENV_FILE) — run make up"; exit 1; fi
	@code=$$(curl -s -o /dev/null -w '%{http_code}' "$(UI_URL)/" 2>/dev/null || echo 000); \
	if [ "$$code" = "000" ]; then echo "UI not reachable at $(UI_URL) — start: make run"; exit 1; fi
	$(NPM) run test:browser

ui-install:
	cd ui && $(NPM) install

ui-build: ui-install
	cd ui && $(NPM) run build

ui-test: ui-install
	cd ui && $(NPM) test

## Role SDL for UI codegen (GraphqlEngine::sdl_for_role → ui/schema/*.graphql)
export-sdl:
	cargo run -q -p e2e-runner --bin e2e-export-sdl -- ui/schema/user.graphql
	GQL_ROLE=admin cargo run -q -p e2e-runner --bin e2e-export-sdl -- ui/schema/admin.graphql

## Export role SDL + regenerate TypedDocumentNode from co-located *.gql
## Codegen uses schema/admin.graphql (field superset incl. admin-only mutations).
## Runtime ACL is still the session engine role — user cannot call admin-only fields.
gen-gql: export-sdl ui-install
	cd ui && $(NPM) run gen:gql

## Fail if engine SDL / generated TS drift from git (CI + agents).
## Paths are relative to the Distributed git root (this worktree / repo).
check-gql: gen-gql
	@set -e; \
	ROOT=$$(git rev-parse --show-toplevel); \
	if ! git -C "$$ROOT" diff --exit-code -- \
		tests/e2e-ui/ui/schema \
		tests/e2e-ui/ui/src/lib/gql/generated \
		tests/e2e-ui/ui/src/routes/todos/todos.generated.ts \
		tests/e2e-ui/ui/src/routes/chat/chat.generated.ts \
		tests/e2e-ui/ui/src/routes/admin/admin.generated.ts; then \
		echo "ERROR: GraphQL schema/codegen drift — run make gen-gql and commit"; \
		git -C "$$ROOT" diff --stat -- tests/e2e-ui/ui/schema tests/e2e-ui/ui/src/routes tests/e2e-ui/ui/src/lib/gql/generated || true; \
		exit 1; \
	fi; \
	echo "check-gql: schema + generated types clean"

## Command catalog from graphql_commands() (same registry as the engine).
export-commands:
	cargo run -q -p e2e-runner --bin e2e-export-commands -- ui/src/lib/api/commands.manifest.json

## Manifest → TypeScript command functions (todosCreate, todosComplete, …).
gen-commands: export-commands ui-install
	cd ui && $(NPM) run gen:commands

## Fail if command manifest / generated client drift from git.
check-commands: gen-commands
	@set -e; \
	ROOT=$$(git rev-parse --show-toplevel); \
	if ! git -C "$$ROOT" diff --exit-code -- \
		tests/e2e-ui/ui/src/lib/api/commands.manifest.json \
		tests/e2e-ui/ui/src/lib/api/commands.generated.ts \
		tests/e2e-ui/ui/src/lib/api/commands.operations.gql \
		tests/e2e-ui/ui/src/lib/api/commands.policies.generated.ts; then \
		echo "ERROR: command client drift — run make gen-commands and commit"; \
		git -C "$$ROOT" diff --stat -- tests/e2e-ui/ui/src/lib/api || true; \
		exit 1; \
	fi; \
	echo "check-commands: manifest + command client clean"

check: check-gql check-commands
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
	@echo "  make export-sdl      role SDL → ui/schema/{user,admin}.graphql"
	@echo "  make gen-gql         export-sdl + ui npm run gen:gql"
	@echo "  make check-gql       gen-gql + fail on schema/generated drift"
	@echo "  make export-commands command manifest from graphql_commands()"
	@echo "  make gen-commands    manifest → TypeScript command functions"
	@echo "  make check-commands  gen-commands + fail on client drift"
	@echo "  make down       docker compose down"
	@echo "  GRAPHIQL=0      disable GraphiQL when running the API"
