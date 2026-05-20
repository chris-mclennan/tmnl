# Shell integration

tmnl's shell mode hosts your real shell, so some "smart terminal"
features are best set up in the shell itself, and some need tmnl and the
shell to cooperate. This page covers both.

## Inline autosuggestions (works today, no tmnl setup)

The fastest way to get fish-style "ghost text" — a greyed-out completion
after your cursor, drawn from history — is the shell's own plugin. tmnl
runs your real shell, so `zsh-autosuggestions` just works inside it.

Install with Homebrew:

```bash
brew install zsh-autosuggestions
echo 'source $(brew --prefix)/share/zsh-autosuggestions/zsh-autosuggestions.zsh' >> ~/.zshrc
```

Open a new shell tab. As you type, a suggestion appears in grey; press
**→** (Right arrow) to accept it, or keep typing to ignore it.

This is the recommended option for autosuggestions right now. tmnl will
grow its own native suggestions later (see `FEATURES.md`), but the shell
plugin is mature and costs nothing to adopt.

## Semantic prompt marks (OSC 133)

tmnl can track when a command is running and when it finishes — if your
shell emits **OSC 133** "semantic prompt" sequences. This is the
foundation for command-aware features (and, later, tmnl's own
autosuggestions).

### Install (zsh)

Add this near the **end** of your `~/.zshrc`:

```bash
source /path/to/tmnl/shell-integration/tmnl.zsh
```

Replace `/path/to/tmnl` with wherever you cloned the repo. Source it
*after* any prompt framework (oh-my-zsh, powerlevel10k, starship) so the
prompt-end mark survives. Open a new shell tab to pick it up.

The snippet is safe to source unconditionally — outside tmnl, every
modern terminal ignores unknown OSC codes, so the sequences are inert.

### What it does

The snippet emits four marks around your prompt and commands:

| Mark | Emitted | Meaning |
|---|---|---|
| `A` | before each prompt | a fresh prompt is about to draw |
| `B` | end of the prompt (`PS1`) | command input begins here |
| `C` | after Enter | command submitted; output begins |
| `D` | before the next prompt | the command finished |

tmnl scans these out of the pty stream before the terminal parser sees
them. Today it uses the `C`/`D` pair to know whether a command is
running — which lets it, for example, skip polling `ps` for a foreground
process name while the shell is idle at a prompt.

If the snippet isn't installed, no marks arrive and tmnl falls back to
its previous behavior. Nothing breaks; you just don't get the
command-aware features.

### AI command completion (⌘I / ⌘K)

With the snippet installed, tmnl adds two local, offline AI features in
shell mode (`fim-engine` — qwen2.5-coder, run in-process; nothing leaves
your machine):

- **⌘I — continuation.** Completes the command you've half-typed. The
  suggestion appears as dim ghost text after the cursor.
- **⌘K — describe a command.** Type what you want in plain English on
  the prompt (e.g. `find files over 100MB`), press ⌘K, and tmnl previews
  a shell command on the line below.

In both, press **Tab** to accept or any other key to dismiss.

Both rely on the OSC 133 `B` mark to find where your command line
begins, so they only work with this snippet installed.

The first AI action of a session loads the model (a one-time ~1 GB
download if it isn't already cached, then ~1 s to load); actions after
that take roughly 0.3–1.6 s on CPU.

### Robustness notes

- The `C`/`D` marks come from zsh's `precmd` / `preexec` hooks and are
  robust regardless of your prompt framework.
- The `B` mark is appended to `PS1`. If a later line in your `.zshrc`
  reassigns `PS1`, the `B` mark is lost — hence "source last." The
  `C`/`D` lifecycle tracking survives that, but ⌘I AI completion needs
  `B`, so source the snippet after your prompt framework.
- tmnl identifies itself as `TERM_PROGRAM=tmnl` if you ever want to gate
  shell config on running inside tmnl.

### Other shells

Only a zsh snippet ships today. bash (via `PROMPT_COMMAND` + a `DEBUG`
trap) and fish (via `fish_prompt` / `fish_preexec` events) emit the same
OSC 133 sequences and would be parsed identically — they just aren't
written yet. See `FEATURES.md`.
