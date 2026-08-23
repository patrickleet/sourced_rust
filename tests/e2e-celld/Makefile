# Celld GraphQL example — sibling of tests/e2e-ui, not make run there.
#
#   make -C tests/e2e-ui up-celld-nats   # Azurite + celld + NATS
#   make run                             # this GraphQL host + the e2e-ui Svelte app

.PHONY: run stop help wasm

BIND         ?= 127.0.0.1:8791
API_PORT     ?= 8791
UI_PORT      ?= 5180
UI_HOST      ?= localhost
UI_URL       ?= http://localhost:5180
ENV_FILE     ?= ../e2e-ui/e2e-ui.env
UI_DIR       ?= ../e2e-ui/ui
CELLD_HTTP_PORT ?= 18080
CELLD_URL    ?= http://127.0.0.1:$(CELLD_HTTP_PORT)
NPM          ?= npm

wasm:
	$(MAKE) -C ../e2e-ui wasm

run: wasm
	@set -e; \
	if [ -f $(ENV_FILE) ]; then set -a; . ./$(ENV_FILE); set +a; fi; \
	export DATABASE_URL="$${E2E_CELLD_DATABASE_URL:-sqlite:./e2e-celld.db?mode=rwc}"; \
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
	export PUBLIC_E2E_PROFILE="celld-nats"; \
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
	echo "starting e2e-celld API on $$_base (CELLD_URL=$$CELLD_URL) …"; \
	cargo run -p e2e-celld-runner --bin e2e-celld > .make-runner.log 2>&1 & \
	echo $$! > .make-runner.pid; \
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
	cleanup() { \
		[ -f .make-ui.pid ] && kill $$(cat .make-ui.pid) 2>/dev/null || true; \
		[ -f .make-runner.pid ] && kill $$(cat .make-runner.pid) 2>/dev/null || true; \
		lsof -ti:$${_api_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		lsof -ti:$${_ui_port} 2>/dev/null | xargs kill -9 2>/dev/null || true; \
		rm -f .make-runner.pid .make-ui.pid; \
	}; \
	trap cleanup EXIT INT TERM; \
	echo ""; \
	echo "  UI     $$_ui   (celld badge in the navbar)"; \
	echo "  API    $$_base"; \
	echo "  CELLD  $$CELLD_URL"; \
	echo "  GraphiQL $$_base/graphql"; \
	echo "  this is tests/e2e-celld — not tests/e2e-ui make run"; \
	echo "  Ctrl-C stops both"; \
	echo ""; \
	wait $$(cat .make-ui.pid) 2>/dev/null || wait

stop:
	@if [ -f .make-ui.pid ]; then kill $$(cat .make-ui.pid) 2>/dev/null || true; fi
	@if [ -f .make-runner.pid ]; then kill $$(cat .make-runner.pid) 2>/dev/null || true; fi
	@lsof -ti:$(API_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@lsof -ti:$(UI_PORT) 2>/dev/null | xargs kill -9 2>/dev/null || true
	@rm -f .make-runner.pid .make-ui.pid
	@echo stopped

help:
	@echo "e2e-celld (new example — not tests/e2e-ui)"
	@echo "  make run   GraphQL host + e2e-ui Svelte app (Todo create/complete → celld)"
	@echo "  make stop  stop API + UI"
	@echo "  infra:     make -C ../e2e-ui up-celld-nats"
