#!/usr/bin/env bash
# Sends controller actions to a Hearth started with --dev-socket, e.g.
#   scripts/send-action.sh home_short
#   scripts/send-action.sh down down accept
# Actions: up down left right accept back prev_tab next_tab options home_short home_long
# Commands (their reply is printed):
#   sound:<name>    play a logical sound (move, accept, …, achievement, mastery, startup)
#   audio_state     the audio engine's state as JSON (playing music and its source, volumes)
set -euo pipefail
cd "$(dirname "$0")/.."
SOCKET="${HEARTH_DEV_SOCKET:-./.dev-data/actions.sock}"
for action in "$@"; do
    python3 -c 'import os, socket, sys
path, word = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
reply = word == "audio_state" or word.startswith("sound:")
if reply:
    # A bound address so Hearth can answer.
    me = os.path.join(os.path.dirname(path) or ".", ".reply-%d.sock" % os.getpid())
    s.bind(me)
    s.settimeout(3)
try:
    s.sendto(word.encode(), path)
    if reply:
        print(s.recv(1 << 20).decode())
finally:
    if reply:
        os.unlink(me)' "$SOCKET" "$action"
    sleep 0.15
done
