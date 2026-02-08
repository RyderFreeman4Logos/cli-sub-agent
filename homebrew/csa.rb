class Csa < Formula
  desc "Recursive Agent Container: Standardized CLI for LLM tools"
  homepage "https://github.com/RyderFreeman4Logos/cli-sub-agent"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/RyderFreeman4Logos/cli-sub-agent/releases/download/v#{version}/csa-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM64_DARWIN"
    else
      url "https://github.com/RyderFreeman4Logos/cli-sub-agent/releases/download/v#{version}/csa-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_X86_64_DARWIN"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/RyderFreeman4Logos/cli-sub-agent/releases/download/v#{version}/csa-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM64_LINUX"
    else
      url "https://github.com/RyderFreeman4Logos/cli-sub-agent/releases/download/v#{version}/csa-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_X86_64_LINUX"
    end
  end

  def install
    bin.install "csa"
  end

  test do
    assert_match "csa", shell_output("#{bin}/csa --version")
  end
end
