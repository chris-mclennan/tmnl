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

# Outputs "branch±" / "branch" / "" — caller checks for empty.
_mnml_seg_git() {
    command -v git >/dev/null 2>&1 || return
    local branch
    branch=$(git symbolic-ref --short HEAD 2>/dev/null) || \
        branch=$(git rev-parse --short HEAD 2>/dev/null) || return
    local dirty=""
    if ! git diff --quiet --ignore-submodules HEAD 2>/dev/null; then
        dirty="±"
    fi
    printf '%s%s' "$branch" "$dirty"
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
_mnml_build_left() {
    local last_exit=$1
    local out=""
    local cwd_text
    cwd_text=$(_mnml_seg_cwd)

    # cwd segment: accent bg, bg-color fg.
    out+="$(_mnml_bg "$MNML_PROMPT_ACCENT")$(_mnml_fg "$MNML_PROMPT_BG") ${cwd_text} "

    local git_text
    git_text=$(_mnml_seg_git)
    if [ -n "$git_text" ]; then
        local git_bg="$MNML_PROMPT_GREEN"
        if [ "$(_mnml_seg_git_dirty)" = "1" ]; then
            git_bg="$MNML_PROMPT_YELLOW"
        fi
        # accent-fg arrow → git bg.
        out+="$(_mnml_bg "$git_bg")$(_mnml_fg "$MNML_PROMPT_ACCENT")${_mnml_sep}"
        out+="$(_mnml_bg "$git_bg")$(_mnml_fg "$MNML_PROMPT_BG") ${_mnml_branch} ${git_text} "
        # git bg → reset (terminal bg).
        out+="${_mnml_reset}$(_mnml_fg "$git_bg")${_mnml_sep}${_mnml_reset}"
    else
        out+="${_mnml_reset}$(_mnml_fg "$MNML_PROMPT_ACCENT")${_mnml_sep}${_mnml_reset}"
    fi

    # Last-exit indicator: red ❯ when non-zero, green otherwise.
    local arrow_fg="$MNML_PROMPT_GREEN"
    if [ "$last_exit" != "0" ] && [ -n "$last_exit" ]; then
        arrow_fg="$MNML_PROMPT_RED"
        out+=" $(_mnml_fg "$MNML_PROMPT_RED")[$last_exit]${_mnml_reset}"
    fi
    out+=" $(_mnml_fg "$arrow_fg")${_mnml_arrow}${_mnml_reset} "

    printf '%s' "$out"
}

_mnml_build_right() {
    local out=""
    local context_label="${MNML_CONTEXT:-$( basename "${SHELL:-sh}" )}"
    out+="$(_mnml_fg "$MNML_PROMPT_GREY")$(date +%H:%M) · ${context_label}${_mnml_reset}"
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
