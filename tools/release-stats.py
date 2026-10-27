#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Summarize GitHub release asset downloads per channel and release.

Every package channel except the Flatpak repository and Nix fetches its
artifact from GitHub Releases: Homebrew, Scoop, WinGet, the AUR package, and
direct downloads. Asset counts describe distribution activity, not unique
users or installations. They count upgrades as well as first installs, and
include automated fetches (WinGet validation and malware scans);
the v0.1.0 MSI used by CI is reported separately because its count mixes
automated and manual downloads.

Uses the GitHub CLI (`gh`) and its authentication.

usage: release-stats.py [--repo OWNER/NAME] [--json]
"""

import argparse
import json
import subprocess
import sys
from collections import defaultdict

# The upgrade test in CI installed this MSI on every Windows run until the
# workflow cached it. Its count mixes CI and manual downloads; GitHub does
# not expose enough information to separate them.
BASELINE_ASSET = ("v0.1.0", "clipboard-transformer-0.1.0-x86_64.msi")

# First match wins; order the more specific suffixes first.
CHANNELS = [
    ("macOS Homebrew", "-apple-darwin-homebrew.zip"),
    ("macOS DMG", ".dmg"),
    ("macOS app ZIP", ".app.zip"),
    ("Linux Homebrew", "-linux-homebrew.tar.xz"),
    ("CLI archive", "clipboard-transformer-cli-"),
    ("Windows MSI", ".msi"),
    ("Windows portable", "-windows-portable.zip"),
    ("Windows portable", ".exe"),
    ("Flatpak bundle", ".flatpak"),
    ("AppImage", ".AppImage"),
    ("DEB", ".deb"),
    ("RPM", ".rpm"),
    ("Pacman", ".pkg.tar.zst"),
    ("Source", ".tar.gz"),
]


def channel(name):
    if name.endswith(".sha256") or name in ("LICENSE", "PKGBUILD"):
        return None
    for label, marker in CHANNELS:
        if marker in name:
            return label
    return "Other"


def releases(repo):
    result = subprocess.run(
        ["gh", "api", "--paginate", f"repos/{repo}/releases",
         "--jq", ".[] | {tag: .tag_name, assets: [.assets[] | {name, download_count}]}"],
        check=True, capture_output=True, text=True,
    )
    return [json.loads(line) for line in result.stdout.splitlines() if line]


def summarize(repo):
    counts = defaultdict(lambda: defaultdict(int))
    baseline_downloads = 0
    tags = []
    for release in releases(repo):
        tags.append(release["tag"])
        for asset in release["assets"]:
            label = channel(asset["name"])
            if label is None:
                continue
            if (release["tag"], asset["name"]) == BASELINE_ASSET:
                baseline_downloads += asset["download_count"]
                continue
            counts[label][release["tag"]] += asset["download_count"]
    tags.sort(key=lambda tag: [int(part) for part in tag.lstrip("v").split(".")
                               if part.isdigit()])
    return tags, counts, baseline_downloads


def print_table(tags, counts, baseline_downloads):
    rows = sorted(counts, key=lambda label: -sum(counts[label].values()))
    width = max(map(len, rows), default=7) + 2
    print("".ljust(width) + "".join(tag.rjust(8) for tag in tags) + "   total")
    for label in rows:
        cells = "".join(str(counts[label].get(tag, 0) or "·").rjust(8) for tag in tags)
        print(label.ljust(width) + cells + str(sum(counts[label].values())).rjust(8))
    per_tag = "".join(str(sum(counts[label].get(tag, 0) for label in rows)).rjust(8)
                      for tag in tags)
    total = sum(sum(row.values()) for row in counts.values())
    print("total".ljust(width) + per_tag + str(total).rjust(8))
    print(f"\nExcluded baseline MSI: {baseline_downloads} downloads of {BASELINE_ASSET[1]}.")
    print("This count mixes CI and manual downloads; their shares are unknown.")
    print("Counts include upgrades and automated fetches; the Flatpak "
          "repository and Nix are not visible here.")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", default="jag-k/clipboard-transformer")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a table")
    args = parser.parse_args()
    try:
        tags, counts, baseline_downloads = summarize(args.repo)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"release-stats: gh api failed: {error}", file=sys.stderr)
        return 1
    if args.json:
        json.dump(
            {"releases": tags, "channels": counts,
             "excluded_baseline_downloads": baseline_downloads},
            sys.stdout, indent=2,
        )
        print()
    else:
        print_table(tags, counts, baseline_downloads)
    return 0


if __name__ == "__main__":
    sys.exit(main())
