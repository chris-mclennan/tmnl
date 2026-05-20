# tmnl shell integration — OSC 133 "semantic prompt" marks (zsh).
#
# Source this near the END of your ~/.zshrc (after any prompt framework
# such as oh-my-zsh, powerlevel10k, or starship has set PS1):
#
#     source /path/to/tmnl/shell-integration/tmnl.zsh
#
# It emits OSC 133 sequences so tmnl can tell when a command is running
# and where your prompt ends. Outside tmnl the sequences are inert —
# every modern terminal ignores unknown OSC codes — so sourcing it
# unconditionally is safe.
#
# What tmnl currently uses: the C/D marks (command running / finished),
# emitted from the precmd/preexec hooks. Those are robust regardless of
# your prompt framework. The B mark (end of prompt) is appended to PS1
# and may be lost if a later line reassigns PS1 — hence "source last".

# Guard against double-sourcing.
if [[ -n "${TMNL_SHELL_INTEGRATION:-}" ]]; then
  return 0
fi
typeset -g TMNL_SHELL_INTEGRATION=1

# Emit one OSC 133 sequence with the given body (e.g. "A", "C", "D;0").
__tmnl_osc133() { printf '\033]133;%s\007' "$1"; }

# precmd runs just before each prompt is drawn.
__tmnl_precmd() {
  local ret=$?
  __tmnl_osc133 "D;$ret"   # the previous command finished, status $ret
  __tmnl_osc133 "A"        # a fresh prompt is about to be drawn
}

# preexec runs after Enter, just before the command runs.
__tmnl_preexec() { __tmnl_osc133 "C"; }

autoload -Uz add-zsh-hook
add-zsh-hook precmd  __tmnl_precmd
add-zsh-hook preexec __tmnl_preexec

# Mark the end of the prompt (B) so tmnl knows where input begins.
# The %{...%} wrapper tells zsh the bytes are zero-width.
PS1="${PS1}%{$(__tmnl_osc133 B)%}"
