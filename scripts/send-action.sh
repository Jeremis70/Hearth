#!/usr/bin/env bash
# Sends controller actions to a Hearth started with --dev-socket, e.g.
#   scripts/send-action.sh home_short
#   scripts/send-action.sh down down accept
# Actions: up down left right accept back prev_tab next_tab options home_short home_long
# Commands (their reply is printed):
#   sound:<name>    play a logical sound (move, accept, …, achievement, mastery, startup)
#   audio_state     the audio engine's state as JSON (playing music and its source, volumes)
#   Customize page (reply "ok", an error, or JSON):
#     customize:<art key>:<art|theme|match>   open it, e.g. customize:gc/GZ2E01:art
#     pick:<n>          use candidate n of the section shown (an image, or a game to match)
#     rematch:<n>       match game n of "Wrong game?"
#     pick_file:<n>     use file n of the file list (images; theme_file:<n> for the theme)
#     position:<0..1>   the hero's crop position (saved)
#     reset:<kind>      back to automatic (grid, hero, logo, icon)
#     theme_start:<s>   theme_remove
#     custom_state      the page and the game's picks as JSON
#   Controllers (fake ones, for --no-pad-input runs):
#     pad:connect:<dualsense|ds4|xbox|switch|generic>   one more (id dev:<kind>)
#     pad:disconnect[:<kind>]                            the last one, or that one
#   Animated art:
#     anim:on|off           Animated artwork (saved, like the Settings toggle)
#     reduce_motion:on|off  Reduce motion (saved)
#     anim_state            what animates now as JSON: per track (hero, logo, preview) the
#                           source, format, mode (full/window/stream), frames, decoded bytes,
#                           flips and late flips; stills and why; caps; .anim disk use
#   Profiles:
#     profile_state         the active profile (id, name) and its folders (saves, controls,
#                           emulators), the startup migration's result, whether games may start
set -euo pipefail
cd "$(dirname "$0")/.."
SOCKET="${HEARTH_DEV_SOCKET:-./.dev-data/actions.sock}"
for action in "$@"; do
    python3 -c 'import os, socket, sys
path, word = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
actions = {"up", "down", "left", "right", "accept", "back", "prev_tab", "next_tab",
           "options", "home_short", "home_long", "capture"}
reply = word not in actions
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
