# Download metrics

The application has no telemetry and should not get any: it reads the
clipboard, and its users would rightly distrust it phoning home. Usage is
estimated from what the distribution channels already record.

## GitHub release assets

Homebrew (macOS and Linux), Scoop, WinGet, the AUR package, and direct
downloads all fetch their artifact from GitHub Releases, so release asset
download counts are the main signal:

```sh
just release-stats
just release-stats --json
```

The table groups assets by channel and release and leaves out checksums,
`LICENSE`, and `PKGBUILD`. These are download counts, not installation or
unique-user counts; cached downloads can also bypass GitHub:

- Upgrades can download the asset again, so the counts include upgrades as
  well as first installs.
- Automated fetches, such as package validation and malware scans, are
  included. Similar counts across assets can suggest automated downloads,
  but the counters alone do not identify who downloaded them or why.
- The Windows upgrade test installs the v0.1.0 MSI. CI fetched it on every run
  before caching was added, so `release-stats` excludes this asset from the
  main totals and reports it separately. Its count mixes CI and manual
  downloads; their shares are unknown. JSON calls this field
  `excluded_baseline_downloads`.

The repository traffic API (`gh api repos/jag-k/clipboard-transformer/traffic/clones`)
covers only 14 days and can include CI and Dependabot clones.

## Channels without numbers

The signed Flatpak repository is published to GitHub Pages at
`flatpak.jag-k.dev`. Its installs and updates are not represented by GitHub
release asset counters. The workflow's download of the release bundle can
itself increase that bundle's count. Nix cache fetches are also outside this
report; check the `jag-k` Cachix dashboard for available metrics.

A future proxy with request metrics could help estimate Flatpak repository
activity. Requests for `summary` can indicate update checks; requests below
`objects/` and `deltas/` can indicate content downloads. Neither directly
counts users or completed installs. Before changing hosting or DNS, verify
the provider's available metrics, caching behavior, and TLS requirements.
