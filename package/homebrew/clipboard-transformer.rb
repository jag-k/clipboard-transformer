# typed: strict
# frozen_string_literal: true

cask "clipboard-transformer" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.1.0"
  sha256 arm:   "ARM_SHA256",
         intel: "INTEL_SHA256"

  url "https://github.com/jag-k/clipboard-transformer/releases/download/v#{version}/clipboard-transformer-#{version}-#{arch}-apple-darwin-homebrew.zip"
  name "Clipboard Transformer"
  desc "Rule-based clipboard transformer"
  homepage "https://github.com/jag-k/clipboard-transformer"

  depends_on macos: ">= :ventura"

  app "Clipboard Transformer.app"
  binary "clipboard-transformer"

  caveats <<~EOS
    Run on Startup is controlled from the app's tray menu.
  EOS
end
