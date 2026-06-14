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

## Prebuilt binaries (curl install)

Binaries are built by `.github/workflows/release.yml`. The workflow runs from
the default branch when a GitHub Release is **published** (or via manual
`workflow_dispatch` with a tag), checks out the tagged code, and uploads
`diffview-<target>.tar.gz` assets for:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

To cut a release with binaries:

```bash
git tag v0.1.0 && git push origin v0.1.0   # if not already pushed
gh release create v0.1.0 --generate-notes   # publishing triggers the build
```

`install.sh` downloads the matching asset from the latest release.

## Homebrew Formula

The formula is in `Formula/diffview.rb` and is published to the
`icankeep/homebrew-diffview` tap.

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
