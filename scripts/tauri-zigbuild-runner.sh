#!/usr/bin/env bash
set -euo pipefail

# Tauri passes cargo-style subcommands to --runner (e.g. "build").
# cargo-zigbuild expects "zigbuild" for build operations.
if [[ $# -gt 0 && "$1" == "build" ]]; then
  shift
  exec cargo-zigbuild zigbuild "$@"
fi

exec cargo-zigbuild "$@"
