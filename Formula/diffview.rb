class Diffview < Formula
  desc "Open an IDE-style side-by-side git diff viewer in your terminal"
  homepage "https://github.com/icankeep/diffview"
  url "https://github.com/icankeep/diffview/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "d49e8d339138583d9008967e04c024427875d56e932f410bdb0bdb98600f2fde"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  def caveats
    <<~EOS
      Configure your preferred terminal before first use:
        diffview config set-terminal iterm2

      Supported terminal values:
        tmux, wezterm, kitty, ghostty, alacritty, iterm2, terminal
    EOS
  end

  test do
    assert_match "Open diffview", shell_output("#{bin}/diffview --help")
    assert_match "IDE-style", shell_output("#{bin}/diffview-tui --help")
  end
end
