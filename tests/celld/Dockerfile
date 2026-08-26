# Local-only: official celld plus socat so Azurite is reachable at
# 127.0.0.1:10000 (object_store emulator client). Not a production image.
FROM ghcr.io/denoland/celld:latest

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends socat \
    && rm -rf /var/lib/apt/lists/*
