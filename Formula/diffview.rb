class Diffview < Formula
  desc "Open an IDE-style side-by-side git diff viewer in your terminal"
  homepage "https://github.com/icankeep/diffview"
  url "https://github.com/icankeep/diffview/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "dd2f767a63acaa53798b284cf6bcd7fd5e8c38cc14dd6fd9c9a9e5b14b15d35c"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  def caveats
    <<~EOS
      Auto-detect and set your terminal from the current environment:
      根据当前环境自动检测并设置预览终端：
        diffview config init

      Or choose it explicitly / 或手动指定:
        diffview config set-terminal iterm2

      Supported terminal values / 支持的终端:
        tmux, wezterm, kitty, ghostty, alacritty, iterm2, terminal
    EOS
  end

  test do
    assert_match "Open diffview", shell_output("#{bin}/diffview --help")
    assert_match "IDE-style", shell_output("#{bin}/diffview-tui --help")
  end
end
