cask "better-kumo" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/IgorKravtsov/better-kumo-app/releases/download/v#{version}/Better-Kumo-macOS-#{version}.zip"
  name "Better Kumo"
  desc "Control your Kumospace desktop from your phone"
  homepage "https://github.com/IgorKravtsov/better-kumo-app"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :ventura"

  app "Better Kumo.app"

  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Better Kumo.app"]
  end

  uninstall quit: "com.kravtsov.kumowatch.desktop"

  zap trash: [
    "~/.config/kumowatch/relay.toml",
    "~/.local/share/kumowatch-relay",
    "~/Library/Logs/kumowatch",
  ]

  caveats <<~EOS
    Better Kumo is not notarized by Apple; this cask removes the quarantine flag so it opens normally.
    Start it from Applications, then turn on "Start at login" in its menu bar menu.
  EOS
end
