# Local celld 0.4 worker. State persists in the celld-dev-data volume.

.PHONY: build build-relay dev-relay up reload logs down reset test help

REPO_ROOT := $(abspath ../..)
COMPOSE   ?= docker-compose.yml
CELLD_HTTP_PORT ?= 18080
CELLD_URL ?= http://127.0.0.1:$(CELLD_HTTP_PORT)

build:
	@command -v worker-build >/dev/null || { echo "worker-build required: cargo install worker-build"; exit 1; }
	cd worker && worker-build --dev

# celld dev serves one project at a time. Copy the generated consumer bundle
# under the producer project so both deployments use worker/.celld/dev.
build-relay:
	@command -v worker-build >/dev/null || { echo "worker-build required: cargo install worker-build"; exit 1; }
	cd relay-worker && worker-build --dev
	mkdir -p worker/build/relay-worker
	cp -R relay-worker/build/. worker/build/relay-worker

dev-relay: build-relay
	celld dev worker/relay.wrangler.jsonc --host 127.0.0.1 --port 18081 --logs

up: build
	docker compose -f $(COMPOSE) up -d celld
	@echo "celld 0.4 dev: $(CELLD_URL) (state volume: celld-dev-data)"

reload: build
	docker compose -f $(COMPOSE) up -d --force-recreate celld

logs:
	docker compose -f $(COMPOSE) logs -f celld

down:
	docker compose -f $(COMPOSE) down

# Explicit destructive reset. Ordinary down/reload preserves the volume.
reset:
	docker compose -f $(COMPOSE) down -v

test:
	cd $(REPO_ROOT) && CELLD_URL="$(CELLD_URL)" cargo test --test celld -- --nocapture

help:
	@echo "celld 0.4 worker"
	@echo "  make up      build Worker + start celld dev"
	@echo "  make dev-relay build + run the Queue consumer against the same dev store"
	@echo "  make reload  rebuild + recreate celld (preserves data)"
	@echo "  make down    stop celld (preserves data)"
	@echo "  make reset   stop celld and delete local cell/queue data"
	@echo "  make test    run live cell tests"
