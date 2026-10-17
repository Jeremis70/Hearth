#!/usr/bin/env bash
# Sends controller actions to a Hearth started with --dev-socket, e.g.
#   scripts/send-action.sh home_short
#   scripts/send-action.sh down down accept
# Actions: up down left right accept back prev_tab next_tab options home_short home_long
set -euo pipefail
cd "$(dirname "$0")/.."
SOCKET="${HEARTH_DEV_SOCKET:-./.dev-data/actions.sock}"
for action in "$@"; do
    python3 -c 'import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.sendto(sys.argv[2].encode(), sys.argv[1])' "$SOCKET" "$action"
    sleep 0.15
done
