# Local celld worker: rebuild WASM, deploy to Azurite, restart the node.
#
#   make reload   worker-build --dev + celld deploy + compose restart
#   make watch    cargo-watch the above (waits for the first source change)
#
# CELLD_WATCH in docker-compose is the node's SQLite/replication dir, not
# a source watcher. Nodes load a deployment at startup, so deploy is not
# enough — this target restarts the celld container after each deploy.

.PHONY: reload watch ensure-watch help

REPO_ROOT := $(abspath ../..)
COMPOSE   ?= docker-compose.yml
# Public Azurite emulator account (already in compose). Not a secret.
AZURE_STORAGE_USE_EMULATOR    ?= true
AZURE_STORAGE_ACCOUNT_NAME    ?= devstoreaccount1
AZURE_STORAGE_ACCOUNT_KEY     ?= Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==

ensure-watch:
	@command -v cargo-watch >/dev/null || { echo "installing cargo-watch…"; cargo install cargo-watch; }

# Debug wasm is faster to iterate than the --release used by up-celld-nats.
reload:
	@command -v worker-build >/dev/null || { echo "worker-build required: cargo install worker-build"; exit 1; }
	@command -v celld >/dev/null || { echo "celld CLI required: curl -fsSL https://celld.dev/install.sh | sh"; exit 1; }
	cd worker && worker-build --dev
	export AZURE_STORAGE_USE_EMULATOR="$(AZURE_STORAGE_USE_EMULATOR)"; \
	export AZURE_STORAGE_ACCOUNT_NAME="$(AZURE_STORAGE_ACCOUNT_NAME)"; \
	export AZURE_STORAGE_ACCOUNT_KEY="$(AZURE_STORAGE_ACCOUNT_KEY)"; \
	( cd $(REPO_ROOT) && celld deploy tests/celld/worker --bucket az://celld )
	docker compose -f $(COMPOSE) restart celld
	@echo "celld restarted with new worker (nodes load a deployment at startup)"

watch: ensure-watch
	@echo "watching worker + cell_host + todo/chat domain (first change triggers reload)"
	cd worker && cargo watch \
		--postpone \
		-d 2 \
		-w src \
		-w Cargo.toml \
		-w wrangler.jsonc \
		-w ../../../src/microsvc/cell_host \
		-w ../../../src/command_dispatch \
		-w ../../e2e-ui/crates/todo-domain \
		-w ../../e2e-ui/crates/chat-domain \
		-- $(MAKE) -C $(CURDIR) reload

help:
	@echo "celld worker"
	@echo "  make reload  worker-build --dev + celld deploy + restart celld"
	@echo "  make watch   cargo-watch reload (postpone until first change)"
