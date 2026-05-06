# Homebrew formula for nihostt.
#
# Install with:
#   brew tap ekhodzitsky/nihostt https://github.com/ekhodzitsky/nihostt
#   brew install nihostt
#
# The sha256 values below are pinned to the v<version> release tarballs.
# They are refreshed automatically by the .github/workflows/homebrew.yml
# workflow after every successful release.yml run — do not hand-edit
# unless you are backfilling a release that predated that automation.

class Nihostt < Formula
  desc "On-device Japanese speech recognition server powered by ReazonSpeech-k2-v2"
  homepage "https://github.com/ekhodzitsky/nihostt"
  version "0.1.2"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ekhodzitsky/nihostt/releases/download/v0.1.2/nihostt-0.1.2-aarch64-apple-darwin.tar.gz"
      sha256 "67c0a24dc4d5fefb166d1687c30e10f3d31054b2a0ca1a74b1e6e42d7d6c8285"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/ekhodzitsky/nihostt/releases/download/v0.1.2/nihostt-0.1.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "3cf8fa3c4e8c134cd7a909c1cf902699825d566fcb69bb6e1e2c415a75904e6b"
    end
  end

  def install
    bin.install "nihostt"
  end

  def caveats
    <<~EOS
      The ReazonSpeech-k2-v2 model (~155 MB INT8) is downloaded on first run
      into ~/.nihostt/models.

      Quick start:
        nihostt download         # fetches model
        nihostt serve            # starts STT server on 127.0.0.1:9876

      Homepage: https://github.com/ekhodzitsky/nihostt
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/nihostt --version")
  end
end
