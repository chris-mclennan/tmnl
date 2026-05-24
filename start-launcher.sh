#!/usr/bin/env bash
# tmnl interactive launcher — pick a mode from a menu.
# Companion to ./run.sh (which takes static subcommands). Run this when
# you want to be prompted; run ./run.sh <mode> directly when you know
# what you want.
set -u
cd "$(dirname "$0")"

TEAL=$'\033[38;2;83;192;188m'
GREEN=$'\033[38;2;152;195;121m'
GREY=$'\033[38;2;92;99;112m'
BOLD=$'\033[1m'
RST=$'\033[0m'

printf '\n%s%s┌─ tmnl launcher ──────────────────────────────────────┐%s\n' \
    "$BOLD" "$TEAL" "$RST"
printf '%s%s│%s  Pick a mode:                                        %s%s│%s\n' \
    "$BOLD" "$TEAL" "$RST" "$BOLD" "$TEAL" "$RST"
printf '%s%s└──────────────────────────────────────────────────────┘%s\n\n' \
    "$BOLD" "$TEAL" "$RST"

PS3=$'\n'"  ${GREEN}→${RST} pick a number: "
COLUMNS=1
options=(
    "tmnl — shell mode (default; opens with welcome screen + native-app shortcuts)"
    "tmnl — editor mode with mnml as a native tab"
    "tmnl — editor mode with mixr as a native tab"
    "tmnl — headless (no window; scripted stdin)"
    "tmnl — editor mode WITHOUT auto-spawning mnml (debug)"
    "build — debug build"
    "release — release build"
    "test — run cargo test"
    "check — fmt + clippy (matches CI)"
    "quit"
)
select choice in "${options[@]}"; do
    case "$REPLY" in
        1) exec ./run.sh ;;
        2) exec ./run.sh mnml ;;
        3) exec ./run.sh mixr ;;
        4) exec ./run.sh headless ;;
        5) exec ./run.sh no-launch ;;
        6) exec ./run.sh build ;;
        7) exec ./run.sh release ;;
        8) exec ./run.sh test ;;
        9) exec ./run.sh check ;;
        10) echo "bye"; exit 0 ;;
        *) printf '  %sunknown choice %q — try again%s\n' "$GREY" "$REPLY" "$RST" ;;
    esac
done
