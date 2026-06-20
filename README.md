# diffview

`diffview` opens an IDE-style side-by-side git diff viewer in an external
interactive terminal.

It is useful from tools that do not provide a full TTY, such as AI coding
agent command output. The user-facing `diffview` command launches a real
terminal session, then runs `diffview-tui` inside it.

## Install

### Homebrew

```bash
brew tap icankeep/diffview
brew install diffview
```

### curl

Downloads a prebuilt binary for your platform (macOS arm64/x86_64, Linux
arm64/x86_64) into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/icankeep/diffview/main/install.sh | sh
```

### From source

```bash
cargo install --git https://github.com/icankeep/diffview
```

## First Use

The curl installer auto-detects your terminal from the environment and sets it
as the default. To (re)run detection yourself, or after a Homebrew install:

```bash
diffview config init
```

To choose the terminal explicitly:

```bash
diffview config set-terminal iterm2
```

Supported values:

```text
tmux, wezterm, kitty, ghostty, alacritty, iterm2, terminal
```

Then run:

```bash
diffview .
```

You can override the configured terminal for a single launch:

```bash
diffview --terminal terminal .
diffview --terminal tmux .
```

## Binaries

- `diffview`: launcher that opens an external interactive terminal.
- `diffview-tui`: the terminal UI that renders diffs.

Most users should run `diffview`.

## Development

```bash
cargo fmt
cargo test
cargo build --release
```
