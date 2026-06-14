# AGENTS.md

## Project

`diffview` is a Rust terminal UI for viewing git diffs and non-git directory snapshots.

## Commands

- Format: `cargo fmt`
- Test: `cargo test`
- Release build: `cargo build --release`
- Install locally: `cp target/release/diffview target/release/diffview-tui /Users/bytedance/.local/bin/`

## Development Notes

- Keep interaction state in `src/app.rs`; rendering belongs in `src/ui.rs`.
- Keep file collection/parsing in `src/git.rs`.
- Prefer App-level unit tests for keyboard, mouse, search, and navigation behavior.
- Run `cargo fmt` and `cargo test` before claiming a change is complete.
- When updating the installed binary, verify `target/release/diffview` and `/Users/bytedance/.local/bin/diffview` have matching hashes.

## Current Interaction Model

- `/` starts search in the currently focused pane.
- `n` / `N` repeat the active search when a search query exists; otherwise they navigate hunks.
- `[` / `]` jump between visible folders.
- `{` / `}` jump between top-level folders.
- `H` / `L` scroll code horizontally.
- Mouse click selects tree rows or focuses the diff pane.
- Dragging the split between the tree and diff panes resizes the tree pane.

## Launcher

`diffview` is the user-facing launcher. It opens `diffview-tui` in a separate
interactive terminal so it can run from tools that do not provide a full TTY,
such as Codex command output. Users must configure their default terminal once
after installing.

Examples:

- Configure iTerm2: `diffview config set-terminal iterm2`
- Show configured terminal: `diffview config get-terminal`
- Open using the configured terminal: `diffview .`
- Force Terminal.app once: `diffview --terminal terminal .`
- Force tmux once: `diffview --terminal tmux .`

Supported terminals are `tmux`, `wezterm`, `kitty`, `ghostty`, `alacritty`,
`iterm2`, and `terminal`.
