# e2e-ui template — run from this directory.
#
#   make up          # Docker: Postgres + Zitadel + bootstrap env
#   make run         # API + UI (uses e2e-ui.env if present)
#   make test         # offline unit/suite/UI structural
#   make test-browser # Playwright UI e2e (needs make up + make run)
#   make up-celld-nats / test-celld-nats / down-celld-nats
#                    # optional celld+NATS profile (not make run)

.PHONY: all up down run run-api stop test ci-offline test-domain test-suite \
	test-browser test-browser-install js-install js-build wasm ui-install ui-build ui-check ui-test \
	gen-client check-client contracts-check check clean help \
	up-celld-nats down-celld-nats down-celld test-celld-nats

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
JS_DIR       ?= ../../js
CARGO_TEST_FLAGS ?= -- --nocapture

DATABASE_URL ?= sqlite:./e2e-ui.db?mode=rwc

# Optional celld+NATS profile (same UI, not the one-process playground).
REPO_ROOT        := $(abspath ../..)
CELLD_COMPOSE    := ../celld/docker-compose.yml
PROFILE_COMPOSE  := celld-nats-profile/docker-compose.yml
CELLD_HTTP_PORT  ?= 18080
NATS_PORT        ?= 14222
CELLD_URL        ?= http://127.0.0.1:$(CELLD_HTTP_PORT)
NATS_URL         ?= nats://127.0.0.1:$(NATS_PORT)
# Public Azurite emulator account (already in tests/celld compose). Not a secret.
AZURE_STORAGE_USE_EMULATOR    ?= true
AZURE_STORAGE_ACCOUNT_NAME    ?= devstoreaccount1
AZURE_STORAGE_ACCOUNT_KEY     ?= Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==

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

## Optional celld + NATS profile of the same UI. Does NOT replace make run.
## Brings up tests/celld (Azurite + celld) and celld-nats-profile (NATS only).
up-celld-nats:
	@set -e; \
	command -v docker >/dev/null || { echo "docker required"; exit 1; }; \
	command -v celld >/dev/null || { echo "celld CLI required: curl -fsSL https://celld.dev/install.sh | sh"; exit 1; }; \
	command -v worker-build >/dev/null || { echo "worker-build required: cargo install worker-build"; exit 1; }; \
	echo "optional profile — not make run"; \
	echo "NATS: $(PROFILE_COMPOSE)"; \
	if docker compose -f $(PROFILE_COMPOSE) ps --status running --services 2>/dev/null | grep -qx nats; then \
		echo "NATS compose already running on $(NATS_URL)"; \
	elif nc -z 127.0.0.1 $(NATS_PORT) 2>/dev/null; then \
		echo "port $(NATS_PORT) is already in use (not this compose project)."; \
		echo "stop the other listener, or: NATS_PORT=14223 make up-celld-nats"; \
		docker ps --format 'table {{.Names}}\t{{.Ports}}' | grep -E '14222|nats' || true; \
		exit 1; \
	else \
		docker compose -f $(PROFILE_COMPOSE) up -d; \
	fi; \
	echo "celld + Azurite: $(CELLD_COMPOSE)"; \
	docker compose -f $(CELLD_COMPOSE) up -d azurite; \
	docker compose -f $(CELLD_COMPOSE) up --exit-code-from azurite-init azurite-init; \
	export AZURE_STORAGE_USE_EMULATOR="$(AZURE_STORAGE_USE_EMULATOR)"; \
	export AZURE_STORAGE_ACCOUNT_NAME="$(AZURE_STORAGE_ACCOUNT_NAME)"; \
	export AZURE_STORAGE_ACCOUNT_KEY="$(AZURE_STORAGE_ACCOUNT_KEY)"; \
	echo "building Todo cell worker…"; \
	( cd ../celld/worker && worker-build --release ); \
	echo "deploying worker to az://celld…"; \
	( cd $(REPO_ROOT) && celld deploy tests/celld/worker --bucket az://celld ); \
	docker compose -f $(CELLD_COMPOSE) up -d celld; \
	docker compose -f $(CELLD_COMPOSE) restart celld; \
	ok=0; \
	for i in $$(seq 1 40); do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' "$(CELLD_URL)/health" 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ]; then ok=1; break; fi; \
		sleep 0.5; \
	done; \
	if [ "$$ok" != "1" ]; then echo "celld not healthy at $(CELLD_URL)/health"; exit 1; fi; \
	echo ""; \
	echo "  CELLD_URL $(CELLD_URL)"; \
	echo "  NATS_URL  $(NATS_URL)"; \
	echo "  smoke:    make test-celld-nats"; \
	echo "  stop:     make down-celld-nats   (NATS only)"; \
	echo "            make down-celld        (Azurite + celld)"; \
	echo "  playground remains: make run"; \
	echo ""

down-celld-nats:
	docker compose -f $(PROFILE_COMPOSE) down
	-@docker rm -f e2e-ui-celld-nats-optional >/dev/null 2>&1 || true
	@echo "NATS profile stopped. celld/Azurite untouched (make down-celld). playground untouched (make down)."

down-celld:
	docker compose -f $(CELLD_COMPOSE) down
	@echo "celld + Azurite stopped. NATS: make down-celld-nats. playground: make down."

test-celld-nats:
	@echo "optional profile smoke — default make test / make run unchanged"
	cd $(REPO_ROOT) && \
	  CELLD_URL="$(CELLD_URL)" NATS_URL="$(NATS_URL)" \
	  cargo test --test e2e_ui_celld_nats_profile --features graphql,http,sqlite -- --nocapture

test: test-domain test-suite ui-install ui-build ui-check ui-test
	@echo "OK — offline domain + suite + UI build + typecheck + structural tests"

## CI-only orchestration: install/build shared JS prerequisites once, then overlap
## independent Rust and UI pipelines without running UI writers concurrently.
ci-offline: ui-install
	@set -eu; \
	status=0; \
	$(MAKE) test-domain test-suite & rust_pid=$$!; \
	( cd ui && \
		$(NPM) run client:check && \
		$(NPM) run build && \
		$(NPM) run check && \
		$(NPM) test \
	) & ui_pid=$$!; \
	wait $$rust_pid || status=$$?; \
	wait $$ui_pid || status=$$?; \
	[ $$status -eq 0 ] || exit $$status; \
	echo "OK — generated drift + offline Rust/UI suites"

test-domain:
	cargo test -p todo-domain -p chat-domain -p blob-domain $(CARGO_TEST_FLAGS)

test-suite:
	cargo test -p e2e-suite --test behavioral $(CARGO_TEST_FLAGS)

## Browser e2e (Playwright). Requires make up + make run (UI :5180, API :8791).
test-browser-install:
	$(NPM) install
	npx playwright install chromium

test-browser: test-browser-install
	@if [ ! -f $(ENV_FILE) ]; then echo "missing $(ENV_FILE) — run make up"; exit 1; fi
	@code=$$(curl -s -o /dev/null -w '%{http_code}' "$(UI_URL)/" 2>/dev/null || echo 000); \
	if [ "$$code" = "000" ]; then echo "UI not reachable at $(UI_URL) — start: make run"; exit 1; fi
	$(NPM) run test:browser

js-install:
	cd $(JS_DIR) && $(NPM) install --ignore-scripts

js-build: js-install
	cd $(JS_DIR) && $(NPM) run build

## blob-domain pure core → ui/src/lib/blob/pkg (wasm-pack; requires wasm32-unknown-unknown).
wasm:
	@command -v wasm-pack >/dev/null || { echo "wasm-pack required: cargo install wasm-pack"; exit 1; }
	@rustup target list --installed | grep -q wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
	@out="$(CURDIR)/ui/src/lib/blob/pkg"; \
	  wasm-pack build crates/blob-domain --target web --out-dir "$$out" --out-name blob_wasm \
	    -- --no-default-features --features wasm

ui-install: js-build wasm
	cd ui && $(NPM) install

ui-build: ui-install
	cd ui && $(NPM) run build

ui-check: ui-install
	cd ui && $(NPM) run check

ui-test: ui-install
	cd ui && $(NPM) test

## One app-owned config drives Vite development, explicit generation, and CI drift checks.
## Manifests remain ephemeral; compiler-owned output remains committed and inspectable.
gen-client: ui-install
	cd ui && $(NPM) run client:generate

## Byte/file-set drift check through distributed --check; never repairs generated files.
check-client: ui-install
	cd ui && $(NPM) run client:check

## Aggregate read-only contract check (repo root catalog when present).
contracts-check:
	@if [ -f ../../contracts/catalog.json ] || [ -f contracts/catalog.json ]; then \
	  $(MAKE) -C ../.. contracts-check; \
	else \
	  echo "contracts-check: no catalog yet; skipping aggregate gate"; \
	fi

check: check-client
	cargo check --workspace
	cargo test -p todo-domain -p chat-domain -p blob-domain --no-run
	cargo test -p e2e-suite --test behavioral --no-run

clean: stop
	cargo clean
	rm -f .make-runner.log e2e-ui.db
	rm -rf ui/node_modules ui/build ui/.svelte-kit ui/src/lib/blob/pkg

help:
	@echo "e2e-ui"
	@echo "  make up         Postgres + Zitadel + OIDC bootstrap → e2e-ui.env"
	@echo "  make run        API + UI (source e2e-ui.env when present)"
	@echo "  make test       offline suite + UI structural"
	@echo "  make ci-offline CI drift + offline suites with safe pipeline overlap"
	@echo "  make wasm            blob-domain core → ui/src/lib/blob/pkg (wasm-pack)"
	@echo "  make gen-client      typed Service → generated user/admin clients"
	@echo "  make check-client    verify generated artifacts byte-for-byte"
	@echo "  make down       playground docker compose down (not celld/NATS)"
	@echo "  make up-celld-nats    optional celld+NATS profile (not make run)"
	@echo "  make test-celld-nats  cargo test --test e2e_ui_celld_nats_profile"
	@echo "  make down-celld-nats  stop NATS profile only"
	@echo "  make down-celld       stop tests/celld Azurite + celld"
	@echo "  GRAPHIQL=0      disable GraphiQL when running the API"
