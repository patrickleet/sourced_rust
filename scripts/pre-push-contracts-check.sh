#!/usr/bin/env sh
# Opt-in pre-push hook template (do not install automatically).
# Install with:
#   ln -s ../../scripts/pre-push-contracts-check.sh .git/hooks/pre-push
set -eu
cd "$(git rev-parse --show-toplevel)"
make contracts-check
