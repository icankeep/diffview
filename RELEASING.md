# Releasing diffview

## Binaries

The package installs two binaries:

- `diffview`: launcher that opens an external interactive terminal.
- `diffview-tui`: the terminal UI that renders diffs.

Users should run `diffview`, not `diffview-tui`, unless they are already in a
real interactive terminal and want to bypass the launcher.

## First Use

Users must configure their terminal once:

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

## Homebrew Formula

The draft formula is in `Formula/diffview.rb`.

Before publishing:

1. Create and push a release tag, for example `v0.1.0`.
2. Compute the release tarball SHA:

   ```bash
   curl -L https://github.com/icankeep/diffview/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
   ```

3. Replace `REPLACE_WITH_RELEASE_TARBALL_SHA256` in the formula.
4. Test locally:

   ```bash
   brew install --build-from-source ./Formula/diffview.rb
   brew test diffview
   ```

For a public tap, copy the formula into a tap repository such as
`homebrew-diffview/Formula/diffview.rb`.
