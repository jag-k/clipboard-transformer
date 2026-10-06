#!/usr/bin/env python3
"""Write AppStream metainfo with release notes taken from CHANGELOG.md.

The committed metainfo carries only the current `<release>` entry, which
cargo-release bumps. At build time this script replaces its `<releases>`
block with every released CHANGELOG section from that version downward, so
software centers such as KDE Discover can show the full history.

usage: write-release-notes.py METAINFO CHANGELOG OUTPUT
"""

import html
import re
import sys
from pathlib import Path

REPOSITORY = "https://github.com/jag-k/clipboard-transformer"
SECTION = re.compile(r"^## \[(?P<version>[^\]]+)\] - (?P<date>\d{4}-\d{2}-\d{2})$")
CURRENT = re.compile(r'<release version="(?P<version>[^"]+)" date="[^"]+" />')
RELEASES = re.compile(r"^  <releases>\n.*?^  </releases>\n", re.MULTILINE | re.DOTALL)


def inline(text: str) -> str:
    """Convert the inline Markdown used in CHANGELOG.md to AppStream markup."""
    parts = re.split(r"(`[^`]+`)", text)
    out = []
    for part in parts:
        if part.startswith("`") and part.endswith("`") and len(part) > 1:
            out.append(f"<code>{html.escape(part[1:-1], quote=False)}</code>")
            continue
        part = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", part)
        part = html.escape(part, quote=False)
        part = re.sub(r"\*\*(.+?)\*\*", r"<em>\1</em>", part)
        out.append(part)
    return "".join(out)


def parse(changelog: str) -> list[tuple[str, str, list[str]]]:
    """Return (version, date, body lines) for every released section."""
    sections: list[tuple[str, str, list[str]]] = []
    current: list[str] | None = None
    for line in changelog.splitlines():
        if line.startswith(("## ", "<!-- next-url -->")):
            current = None
            match = SECTION.match(line)
            if match:
                current = []
                sections.append((match["version"], match["date"], current))
        elif current is not None:
            current.append(line)
    return sections


def description(lines: list[str]) -> list[str]:
    """Render a section body as AppStream `<p>` and `<ul>` blocks."""
    blocks: list[tuple[str, list[str]]] = []
    for line in lines:
        stripped = line.strip()
        if not stripped:
            if blocks and blocks[-1][0] == "p":
                blocks.append(("break", []))
            continue
        if stripped.startswith("### "):
            blocks.append(("p", [stripped[4:]]))
            blocks.append(("break", []))
        elif stripped.startswith(("- ", "* ")):
            if not blocks or blocks[-1][0] != "ul":
                blocks.append(("ul", []))
            blocks[-1][1].append(stripped[2:])
        elif blocks and blocks[-1][0] == "ul" and line.startswith("  "):
            blocks[-1][1][-1] += " " + stripped
        elif blocks and blocks[-1][0] == "p":
            blocks[-1][1][0] += " " + stripped
        else:
            blocks.append(("p", [stripped]))

    out: list[str] = []
    for kind, items in blocks:
        if kind == "p":
            out.append(f"<p>{inline(items[0])}</p>")
        elif kind == "ul":
            out.append("<ul>")
            out.extend(f"  <li>{inline(item)}</li>" for item in items)
            out.append("</ul>")
    return out


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    metainfo_path, changelog_path, output_path = map(Path, sys.argv[1:])
    metainfo = metainfo_path.read_text(encoding="utf-8")

    current = CURRENT.search(metainfo)
    if current is None:
        print(f"{metainfo_path}: no <release> entry found", file=sys.stderr)
        return 1
    version = current["version"]

    sections = parse(changelog_path.read_text(encoding="utf-8"))
    versions = [section[0] for section in sections]
    if version not in versions:
        print(f"{changelog_path}: no section for {version}", file=sys.stderr)
        return 1

    releases = ["  <releases>"]
    for release, date, body in sections[versions.index(version) :]:
        releases.append(f'    <release version="{release}" date="{date}">')
        notes = description(body)
        if notes:
            releases.append("      <description>")
            releases.extend(f"        {line}" for line in notes)
            releases.append("      </description>")
        releases.append(
            f'      <url type="details">{REPOSITORY}/releases/tag/v{release}</url>'
        )
        releases.append("    </release>")
    releases.append("  </releases>")

    rendered, count = RELEASES.subn("\n".join(releases) + "\n", metainfo)
    if count != 1:
        print(f"{metainfo_path}: expected one <releases> block", file=sys.stderr)
        return 1
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
