#!/bin/sh
set -eu
i=0
while [ "$i" -lt 30 ]; do
  if az storage container create -n celld --connection-string "$AZURE_STORAGE_CONNECTION_STRING"; then
    exit 0
  fi
  i=$((i + 1))
  sleep 2
done
echo "azurite did not accept container create" >&2
exit 1
