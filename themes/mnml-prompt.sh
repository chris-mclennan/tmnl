# mnml-prompt.sh — themed powerline prompt for bash + zsh.
#
# Sourced by the user's `.zshrc` / `.bashrc` via:
#
#     [ -n "$MNML_PROMPT_SCRIPT" ] && source "$MNML_PROMPT_SCRIPT"
#
# `$MNML_PROMPT_SCRIPT` is exported by mnml/tmnl before they spawn a
# shell; outside their child shells the line above is a no-op.
#
# Theming is via env vars — also exported by mnml/tmnl before spawn —
# so the prompt automatically picks up the active mnml palette colors.
# Unset vars fall back to a tokyo-night-ish default so the script
# still looks reasonable if sourced standalone.
#
# Segments (left to right):
#   1. cwd            — accent bg, truncated to ~/proj/subdir
#   2. git branch + dirty marker (±)  — green when clean, yellow when dirty
#   3. exit code (non-zero only)      — red
#
# Right-aligned (zsh RPROMPT; bash via the same PROMPT_COMMAND):
#   4. clock HH:MM    — dim grey
#   5. context chip   — "mnml" / "tmnl" / shell-name, dim grey
#
# Glyphs are powerline + nerd-font:    ±   ❯
#
# Shells handled: bash 3+, zsh 5+. Fish + nu intentionally out of scope.

# --- Defaults (tokyo-night-ish) ---------------------------------------------
: "${MNML_PROMPT_BG:=#1a1b26}"
: "${MNML_PROMPT_FG:=#c0caf5}"
: "${MNML_PROMPT_ACCENT:=#7aa2f7}"
# Subtle dark-grey bg for the cwd chip — matches the "active chip"
# tone in mnml's bufferline + the statusline chip bg. Less shouty
# than painting the chip in the full accent color.
: "${MNML_PROMPT_CHIP_BG:=#292d35}"
# Primary blue chip color — matches mnml's statusline TREE / adx
# pills. Used as the cwd-chip background so the prompt reads like
# a continuation of the family chrome.
: "${MNML_PROMPT_BLUE:=#61afef}"
: "${MNML_PROMPT_GREEN:=#9ece6a}"
: "${MNML_PROMPT_RED:=#f7768e}"
: "${MNML_PROMPT_YELLOW:=#e0af68}"
: "${MNML_PROMPT_GREY:=#565f89}"
: "${MNML_CONTEXT:=}"

# --- Color helpers ----------------------------------------------------------
# zsh + bash need non-printing escape sequences bracketed so the
# shell's column counter doesn't include the byte length of the
# escape. zsh uses `%{…%}`, bash uses `\[…\]`. We detect the active
# shell once at source-time and bake the brackets directly into
# `_mnml_fg`/`_mnml_bg`/`_mnml_reset` so callers never have to
# think about it. Without this, PROMPT renders but zsh thinks it
# is 0 columns wide → RPROMPT mis-aligns + the line gets cleared.
if [ -n "${ZSH_VERSION:-}" ]; then
    _mnml_lb='%{'
    _mnml_rb='%}'
elif [ -n "${BASH_VERSION:-}" ]; then
    _mnml_lb='\['
    _mnml_rb='\]'
else
    _mnml_lb=''
    _mnml_rb=''
fi

# Hex "#RRGGBB" → bracketed ANSI 24-bit fg/bg escape.
_mnml_fg() {
    local h="${1#\#}"
    printf '%s\033[38;2;%d;%d;%dm%s' \
        "$_mnml_lb" "0x${h:0:2}" "0x${h:2:2}" "0x${h:4:2}" "$_mnml_rb"
}
_mnml_bg() {
    local h="${1#\#}"
    printf '%s\033[48;2;%d;%d;%dm%s' \
        "$_mnml_lb" "0x${h:0:2}" "0x${h:2:2}" "0x${h:4:2}" "$_mnml_rb"
}
_mnml_reset="${_mnml_lb}"$'\033[0m'"${_mnml_rb}"

# Powerline + nerd-font glyphs.
_mnml_sep=$''      #  — right-pointing solid arrow (segment end)
_mnml_sep_r=$''    #  — left-pointing solid arrow (right-side seg)
_mnml_branch=$''   #  — branch
_mnml_arrow='❯'

# --- Segment builders -------------------------------------------------------
_mnml_seg_cwd() {
    local p="${PWD/#$HOME/~}"
    # Truncate to the trailing N path components if very deep (keep last 3).
    case "$p" in
        */*/*/*)
            local IFS=/
            # shellcheck disable=SC2206
            local parts=( $p )
            local n=${#parts[@]}
            p=".../${parts[$((n - 3))]}/${parts[$((n - 2))]}/${parts[$((n - 1))]}"
            ;;
    esac
    printf '%s' "$p"
}

# Outputs "branch±↑N↓M" — caller checks for empty. The ahead/
# behind suffix uses `git rev-list --left-right --count` against
# the tracked upstream; absent (no upstream / not a tracking
# branch) ⇒ no arrows.
_mnml_seg_git() {
    command -v git >/dev/null 2>&1 || return
    local branch
    branch=$(git symbolic-ref --short HEAD 2>/dev/null) || \
        branch=$(git rev-parse --short HEAD 2>/dev/null) || return
    local dirty=""
    if ! git diff --quiet --ignore-submodules HEAD 2>/dev/null; then
        dirty="±"
    fi
    local ahead_behind=""
    local ab
    ab=$(git rev-list --left-right --count HEAD...@{upstream} 2>/dev/null)
    if [ -n "$ab" ]; then
        local ahead behind
        ahead=$(printf '%s' "$ab" | awk '{print $1}')
        behind=$(printf '%s' "$ab" | awk '{print $2}')
        if [ "$ahead" -gt 0 ] 2>/dev/null; then
            ahead_behind="${ahead_behind}↑${ahead}"
        fi
        if [ "$behind" -gt 0 ] 2>/dev/null; then
            ahead_behind="${ahead_behind}↓${behind}"
        fi
    fi
    printf '%s%s%s' "$branch" "$dirty" "$ahead_behind"
}

# Now-playing chip — reads ~/.mixr/quick.txt. That file is a
# `key=value` dump, not a plain track name; the relevant fields
# are `playing_active=true|false` (chip-show gate) and
# `playing=<track>` (display text). Mixr writes `—` to `playing`
# when idle. Output empty ⇒ no chip.
_mnml_seg_now_playing() {
    local f="$HOME/.mixr/quick.txt"
    [ -r "$f" ] || return
    local active track
    active=$(awk -F= '$1=="playing_active"{print $2; exit}' "$f" 2>/dev/null)
    [ "$active" = "true" ] || return
    track=$(awk -F= '$1=="playing"{print $2; exit}' "$f" 2>/dev/null)
    track="${track#"${track%%[![:space:]]*}"}"
    track="${track%"${track##*[![:space:]]}"}"
    [ -n "$track" ] && [ "$track" != "—" ] || return
    if [ "${#track}" -gt 28 ]; then
        track="${track:0:27}…"
    fi
    printf '%s' "$track"
}

# Attention chip — reads ~/.cache/tmnl/attention-count.txt (tmnl
# writes the count of attention-flagged tabs each tick). Absent
# or "0" ⇒ no chip.
_mnml_seg_attention() {
    local f="$HOME/.cache/tmnl/attention-count.txt"
    [ -r "$f" ] || return
    local count
    count=$(cat "$f" 2>/dev/null)
    [ -n "$count" ] && [ "$count" != "0" ] || return
    printf '%s' "$count"
}

# Returns "1" when the working tree is dirty, "0" otherwise. Used to
# pick the git segment's bg color (green / yellow).
_mnml_seg_git_dirty() {
    if git diff --quiet --ignore-submodules HEAD 2>/dev/null; then
        printf 0
    else
        printf 1
    fi
}

# --- Left-side builder ------------------------------------------------------
# Constructs the prompt string. The `$1` argument is the last exit
# code (passed in from PROMPT_COMMAND / precmd). Returns a string
# containing raw escape codes — wrap with shell-specific brackets at
# the call site.
#
# Style (2026-06-09): mnml-statusline-style chips — subtle dark-grey
# chip bg with the accent only on text + glyphs. No powerline arrows.
# Reads as a continuation of the bufferline / statusline rather than
# a 90s-Linux-bash prompt.
_mnml_build_left() {
    local last_exit=$1
    local out=""
    local cwd_text
    cwd_text=$(_mnml_seg_cwd)

    # cwd chip: blue bg + dark fg + trailing powerline `` arrow in
    # blue fg on terminal bg. Matches mnml's TREE pill exactly —
    # rounded text region + pointy right end.
    out+="$(_mnml_bg "$MNML_PROMPT_BLUE")$(_mnml_fg "$MNML_PROMPT_BG") ${cwd_text} ${_mnml_reset}"
    out+="$(_mnml_fg "$MNML_PROMPT_BLUE")${_mnml_sep}${_mnml_reset}"

    local git_text
    git_text=$(_mnml_seg_git)
    if [ -n "$git_text" ]; then
        local git_bg="$MNML_PROMPT_GREEN"
        if [ "$(_mnml_seg_git_dirty)" = "1" ]; then
            git_bg="$MNML_PROMPT_YELLOW"
        fi
        # git: matching powerline segment — green when clean, yellow
        # when dirty. Branch glyph + name on the colored bg, then
        # the trailing `` arrow.
        out+=" $(_mnml_bg "$git_bg")$(_mnml_fg "$MNML_PROMPT_BG") ${_mnml_branch} ${git_text} ${_mnml_reset}"
        out+="$(_mnml_fg "$git_bg")${_mnml_sep}${_mnml_reset}"
    fi

    # Last-exit indicator: red `[N]` only when non-zero AND not a
    # "user just hit Ctrl+C on an unsubmitted line" signal exit.
    # Common signal exits are 128 + signo: 130 = SIGINT (Ctrl+C),
    # 131 = SIGQUIT, 137 = SIGKILL, 143 = SIGTERM. None of these
    # are useful to show in the prompt — they reflect user intent
    # or external action, not a real command failure.
    # 127 = command-not-found — typing a typo at the prompt is
    # already self-evident; the red `[127]` chip just adds noise.
    local hide_exit=0
    case "$last_exit" in
        127|130|131|137|143) hide_exit=1 ;;
    esac
    if [ "$last_exit" != "0" ] && [ -n "$last_exit" ] && [ "$hide_exit" -eq 0 ]; then
        out+=" $(_mnml_fg "$MNML_PROMPT_RED")[$last_exit]${_mnml_reset}"
    fi

    # Trailing arrow — green on success, red on real error (signal
    # exits stay green since they aren't really a "failed command").
    local arrow_fg="$MNML_PROMPT_GREEN"
    if [ "$last_exit" != "0" ] && [ -n "$last_exit" ] && [ "$hide_exit" -eq 0 ]; then
        arrow_fg="$MNML_PROMPT_RED"
    fi
    out+=" $(_mnml_fg "$arrow_fg")${_mnml_arrow}${_mnml_reset} "

    printf '%s' "$out"
}

_mnml_build_right() {
    # mnml-statusline-style: a single grey "info" chip on the
    # left (now-playing · attention · clock), then a blue left-
    # pointing powerline arrow into a blue context pill. Reads
    # as a mirror of the left-side cwd → branch chips. Matches
    # the look in mnml's right statusline (image #52 reference).
    local out=""
    # ─── grey info chip ─────────────────────────────────────
    local info=""
    local np
    np=$(_mnml_seg_now_playing)
    if [ -n "$np" ]; then
        info+="♪ ${np}"
    fi
    local at
    at=$(_mnml_seg_attention)
    if [ -n "$at" ]; then
        [ -n "$info" ] && info+="  "
        info+="● ${at}"
    fi
    [ -n "$info" ] && info+="  "
    info+="$(date +%H:%M)"
    # Paint the grey chip — dark-grey bg, dim grey fg.
    out+="$(_mnml_bg "$MNML_PROMPT_CHIP_BG")$(_mnml_fg "$MNML_PROMPT_GREY") ${info} ${_mnml_reset}"
    # ─── blue powerline arrow → blue context pill ──────────
    local context_label="${MNML_CONTEXT:-$( basename "${SHELL:-sh}" )}"
    # The arrow's fg = next chip's bg (blue); the arrow renders
    # on the previous chip's bg (chip_bg), so the apparent
    # transition is grey → blue.
    out+="$(_mnml_bg "$MNML_PROMPT_CHIP_BG")$(_mnml_fg "$MNML_PROMPT_BLUE")${_mnml_sep_r}${_mnml_reset}"
    out+="$(_mnml_bg "$MNML_PROMPT_BLUE")$(_mnml_fg "$MNML_PROMPT_BG") ${context_label} ${_mnml_reset}"
    printf '%s' "$out"
}

# --- Wire to shell ----------------------------------------------------------
if [ -n "${ZSH_VERSION:-}" ]; then
    setopt PROMPT_SUBST
    # No outer %{…%} wrap — the escapes inside `_mnml_build_left`
    # / `_mnml_build_right` are already bracketed by `_mnml_fg`
    # etc. Wrapping the whole prompt would make zsh treat the
    # entire output (visible glyphs included) as zero-width,
    # mis-placing RPROMPT and clearing the line.
    PROMPT='$(_mnml_build_left "$?")'
    RPROMPT='$(_mnml_build_right)'

    # Transient prompt — Pure / Powerlevel10k pattern. Before
    # submitting a command, rewrite the just-finished prompt to
    # drop the RPROMPT (keep the left side intact). Without
    # this, every scrollback line that was once a prompt keeps
    # its RPROMPT chip strip — gets noisy in bottom-prompt mode.
    #
    # Use a UNIQUE widget name + `bindkey '^M'` directly so
    # plugins that wrap `accept-line` (zsh-syntax-highlighting,
    # autosuggestions) don't undo this binding.
    _mnml_transient_accept_line() {
        local saved="$RPROMPT"
        RPROMPT=''
        zle .reset-prompt
        RPROMPT="$saved"
        zle .accept-line
    }
    zle -N _mnml_transient_accept_line
    bindkey '^M' _mnml_transient_accept_line
elif [ -n "${BASH_VERSION:-}" ]; then
    # bash builds the prompt via PROMPT_COMMAND so `$?` is captured
    # before any other command runs. The build functions already
    # emit \[…\] brackets around each escape (set up at script
    # source time when BASH_VERSION was detected), so PS1 just
    # concatenates left + right without additional wrapping.
    _mnml_set_ps1() {
        local last=$?
        local left right
        left=$(_mnml_build_left "$last")
        right=$(_mnml_build_right)
        # Two-line layout: left on row 1, right on row 2 indented.
        # bash has no native RPROMPT so right-alignment without a
        # tput-cols-per-redraw dance gets ugly fast.
        PS1="${left}"$'\n'"${right} "
    }
    case "$PROMPT_COMMAND" in
        *_mnml_set_ps1*) : ;;
        '') PROMPT_COMMAND='_mnml_set_ps1' ;;
        *)  PROMPT_COMMAND="_mnml_set_ps1; $PROMPT_COMMAND" ;;
    esac
fi
