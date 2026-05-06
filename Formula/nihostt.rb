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
  version "0.1.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ekhodzitsky/nihostt/releases/download/v0.1.1/nihostt-0.1.1-aarch64-apple-darwin.tar.gz"
      sha256 "96d879dc59ed8f8120da713a820656e67d3619088b845a8466d991e893a048fa"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/ekhodzitsky/nihostt/releases/download/v0.1.1/nihostt-0.1.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "03411e90223ffa642fdaa7b7ac895eea9aeb543b099c14972ff5e7975617c72f"
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
