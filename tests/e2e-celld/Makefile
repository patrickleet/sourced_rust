# Celld GraphQL example — sibling of tests/e2e-ui, not make run there.
#
#   make -C tests/e2e-ui up-celld-nats   # Azurite + celld + NATS
#   make run                             # this GraphQL host + the e2e-ui Svelte app
#
# make run watches Rust (cargo-watch) and the cell worker; Vite HMR covers
# the UI. WATCH=0 / WATCH_WORKER=0 disable those loops. CELLD_WATCH in
# compose is the node's SQLite dir, not a source watcher.

.PHONY: run stop test help wasm ensure-watch

BIND         ?= 0.0.0.0:8791
API_PORT     ?= 8791
UI_PORT      ?= 5180
UI_HOST      ?= localhost
UI_URL       ?= http://localhost:5180
ENV_FILE     ?= ../e2e-ui/e2e-ui.env
UI_DIR       ?= ../e2e-ui/ui
CELLD_HTTP_PORT ?= 18080
CELLD_URL    ?= http://127.0.0.1:$(CELLD_HTTP_PORT)
NATS_PORT    ?= 14222
NATS_URL     ?= nats://127.0.0.1:$(NATS_PORT)
NPM          ?= npm
WATCH        ?= 1
WATCH_WORKER ?= 1

wasm:
	$(MAKE) -C ../e2e-ui wasm

ensure-watch:
	@command -v cargo-watch >/dev/null || { echo "installing cargo-watch…"; cargo install cargo-watch; }

run: wasm $(if $(filter 1,$(WATCH) $(WATCH_WORKER)),ensure-watch)
	@set -e; \
	if [ -f $(ENV_FILE) ]; then set -a; . ./$(ENV_FILE); set +a; fi; \
	if [ -n "$${E2E_CELLD_DATABASE_URL:-}" ]; then export DATABASE_URL="$$E2E_CELLD_DATABASE_URL"; fi; \
	case "$${DATABASE_URL:-}" in \
	  postgres://*|postgresql://*) ;; \
	  *) \
	    echo "e2e-celld requires Postgres DATABASE_URL — start: make -C ../e2e-ui up"; \
	    echo "or: E2E_CELLD_DATABASE_URL=postgres://… make run"; \
	    exit 1; \
	    ;; \
	esac; \
	if [ -n "$${OIDC_ISSUER:-}" ] && ! curl -sf "$${OIDC_JWKS_URI:-$$OIDC_ISSUER/oauth/v2/keys}" >/dev/null 2>&1; then \
	  echo "OIDC issuer not reachable ($$OIDC_ISSUER) — DevHeaders until: make -C ../e2e-ui up"; \
	  unset OIDC_ISSUER OIDC_AUDIENCE OIDC_JWKS_URI; \
	fi; \
	_celld="$(CELLD_URL)"; \
	code=$$(curl -s -o /dev/null -w '%{http_code}' "$${_celld}/health" 2>/dev/null || echo 000); \
	if [ "$$code" != "200" ]; then \
	  if curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:18880/health" 2>/dev/null | grep -q 200; then \
	    _celld="http://127.0.0.1:18880"; \
	  else \
	    echo "celld not healthy at $$_celld — start: make -C ../e2e-ui up-celld-nats"; \
	    echo "or: CELLD_HTTP_PORT=18880 make run"; \
	    exit 1; \
	  fi; \
	fi; \
	export CELLD_URL="$$_celld"; \
	_nats="$(NATS_URL)"; \
	if ! nc -z 127.0.0.1 $(NATS_PORT) 2>/dev/null; then \
	  echo "NATS not reachable at $$_nats — start: make -C ../e2e-ui up-celld-nats"; \
	  exit 1; \
	fi; \
	export NATS_URL="$$_nats"; \
	export PUBLIC_E2E_PROFILE="celld-nats"; \
	_bind="$${BIND:-0.0.0.0:8791}"; \
	_api_port="$${_bind##*:}"; \
	_base="$${E2E_API_ORIGIN:-http://127.0.0.1:$${_api_port}}"; \
	_ui_port="$(UI_PORT)"; \
	_ui_host="$(UI_HOST)"; \
	_ui="$${E2E_UI_ORIGIN:-http://$${_ui_host}:$${_ui_port}}"; \
	export AUTH_URL="$${_ui}"; \
	export AUTH_USE_SECURE_COOKIES="false"; \
	export BIND="0.0.0.0:$${_api_port}"; \
	lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	rm -f .make-runner.pid .make-ui.pid .make-worker.pid .make-runner.log; \
	echo "starting e2e-celld API on $$_base (DATABASE_URL=$$DATABASE_URL CELLD_URL=$$CELLD_URL NATS_URL=$$NATS_URL) …"; \
	if [ "$(WATCH)" = "1" ]; then \
	  echo "API cargo-watch on (WATCH=0 to disable) …"; \
	  cargo watch -d 1 \
	    -i target -i '*.db' -i '.make-*' \
	    -w crates -w Cargo.toml \
	    -w ../../src -w ../../Cargo.toml \
	    -w ../e2e-ui/crates/todo-domain \
	    -w ../e2e-ui/crates/chat-domain \
	    -w ../e2e-ui/crates/blob-domain \
	    -w ../e2e-ui/crates/readmodels \
	    -w ../e2e-ui/crates/projections \
	    -x "run -p e2e-celld-runner --bin e2e-celld" \
	    > .make-runner.log 2>&1 & \
	else \
	  cargo run -p e2e-celld-runner --bin e2e-celld > .make-runner.log 2>&1 & \
	fi; \
	echo $$! > .make-runner.pid; \
	if [ "$(WATCH_WORKER)" = "1" ]; then \
	  echo "worker cargo-watch on (WATCH_WORKER=0 to disable) …"; \
	  $(MAKE) -C ../celld watch > .make-worker.log 2>&1 & \
	  echo $$! > .make-worker.pid; \
	fi; \
	ok=0; \
	for i in $$(seq 1 240); do \
		code=$$(curl -s -o /dev/null -w '%{http_code}' -X POST "$${_base}/graphql" \
			-H 'content-type: application/json' \
			-d '{"query":"{ __typename }"}' 2>/dev/null || echo 000); \
		if [ "$$code" = "200" ] || [ "$$code" = "401" ]; then ok=1; break; fi; \
		sleep 0.5; \
	done; \
	if [ "$$ok" != "1" ]; then \
		echo "API failed:"; tail -80 .make-runner.log; exit 1; \
	fi; \
	echo "API ready (HTTP probe $$code). starting UI from tests/e2e-ui/ui …"; \
	cd $(UI_DIR) && PUBLIC_E2E_PROFILE=celld-nats E2E_API_ORIGIN="$$_base" $(NPM) run dev -- --host $$_ui_host --port $$_ui_port & \
	echo $$! > $(CURDIR)/.make-ui.pid; \
	cd $(CURDIR); \
	stop_pidfile() { \
		[ -f "$$1" ] || return 0; \
		_pid=$$(cat "$$1"); \
		for _child in $$(pgrep -P "$$_pid" 2>/dev/null); do \
		  pkill -P "$$_child" 2>/dev/null || true; \
		  kill "$$_child" 2>/dev/null || true; \
		done; \
		kill "$$_pid" 2>/dev/null || true; \
		rm -f "$$1"; \
	}; \
	cleanup() { \
		stop_pidfile .make-ui.pid; \
		stop_pidfile .make-runner.pid; \
		stop_pidfile .make-worker.pid; \
		lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	}; \
	trap cleanup EXIT INT TERM; \
	echo ""; \
	echo "  UI     $$_ui   (celld badge in the navbar)"; \
	echo "  API    $$_base"; \
	echo "  CELLD  $$CELLD_URL"; \
	echo "  NATS   $$NATS_URL"; \
	echo "  GraphiQL $$_base/graphql"; \
	echo "  this is tests/e2e-celld — not tests/e2e-ui make run"; \
	echo "  watch: GraphQL cargo-watch, worker rebuild+deploy, Vite HMR"; \
	echo "  Ctrl-C stops API + UI + worker watch"; \
	echo ""; \
	wait $$(cat .make-ui.pid) 2>/dev/null || wait

test:
	cargo test --workspace -- --nocapture

stop:
	@stop_pidfile() { \
		[ -f "$$1" ] || return 0; \
		_pid=$$(cat "$$1"); \
		for _child in $$(pgrep -P "$$_pid" 2>/dev/null); do \
		  pkill -P "$$_child" 2>/dev/null || true; \
		  kill "$$_child" 2>/dev/null || true; \
		done; \
		kill "$$_pid" 2>/dev/null || true; \
		rm -f "$$1"; \
	}; \
	stop_pidfile .make-ui.pid; \
	stop_pidfile .make-runner.pid; \
	stop_pidfile .make-worker.pid; \
	lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true; \
	echo stopped

help:
	@echo "e2e-celld (new example — not tests/e2e-ui)"
	@echo "  make test  cargo test --workspace (CI)"
	@echo "  make run   GraphQL + UI (cargo-watch API, worker reload, Vite HMR)"
	@echo "  WATCH=0    one-shot cargo run (no GraphQL reload)"
	@echo "  WATCH_WORKER=0  skip worker-build + celld deploy watch"
	@echo "  make stop  stop API + UI + worker watch"
	@echo "  infra:     make -C ../e2e-ui up && make -C ../e2e-ui up-celld-nats"
	@echo "  Postgres read models from e2e-ui.env DATABASE_URL (not sqlite)"
