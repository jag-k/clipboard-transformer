#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["mistune==3.3.4"]
# ///
"""Write AppStream metainfo with release notes taken from CHANGELOG.md.

The committed metainfo carries only the current `<release>` entry, which
cargo-release bumps. This script replaces its `<releases>` block with every
released CHANGELOG section from that version downward, so software centers
such as KDE Discover can show the full history.

CHANGELOG.md is parsed with mistune and rendered into the markup AppStream
allows in release descriptions. Constructs AppStream cannot express fail the
script instead of being flattened silently.

usage: write-release-notes.py METAINFO CHANGELOG OUTPUT
"""

import re
import sys
from html import escape
from pathlib import Path

import mistune

REPOSITORY = "https://github.com/jag-k/clipboard-transformer"
SECTION = re.compile(
    r"^\[?(?P<version>\d+\.\d+\.\d+[^\]\s]*)\]? - (?P<date>\d{4}-\d{2}-\d{2})$"
)
CURRENT = re.compile(r'<release version="(?P<version>[^"]+)" date="[^"]+" />')
RELEASES = re.compile(r"^  <releases>\n.*?^  </releases>\n", re.MULTILINE | re.DOTALL)


class UnsupportedMarkdown(ValueError):
    pass


def plain(tokens: list[dict]) -> str:
    """Return the text content of inline tokens."""
    out = []
    for token in tokens:
        kind = token["type"]
        if kind in ("text", "codespan"):
            out.append(token["raw"])
        elif kind in ("softbreak", "linebreak"):
            out.append(" ")
        elif "children" in token:
            out.append(plain(token["children"]))
        else:
            raise UnsupportedMarkdown(f"inline {kind}")
    return "".join(out)


def inline(tokens: list[dict]) -> str:
    """Render inline tokens as AppStream `<em>`/`<code>` markup."""
    out = []
    for token in tokens:
        kind = token["type"]
        if kind == "text":
            out.append(escape(token["raw"], quote=False))
        elif kind == "codespan":
            out.append(f"<code>{escape(token['raw'], quote=False)}</code>")
        elif kind in ("strong", "emphasis"):
            # AppStream does not nest inline markup.
            out.append(f"<em>{escape(plain(token['children']), quote=False)}</em>")
        elif kind == "link":
            out.append(inline(token["children"]))
        elif kind in ("softbreak", "linebreak"):
            out.append(" ")
        else:
            raise UnsupportedMarkdown(f"inline {kind}")
    return "".join(out)


def block(token: dict) -> list[str]:
    """Render one block token of a release section."""
    kind = token["type"]
    if kind == "blank_line":
        return []
    if kind == "heading" and token["attrs"]["level"] == 3:
        return [f"<p>{inline(token['children'])}</p>"]
    if kind == "paragraph":
        return [f"<p>{inline(token['children'])}</p>"]
    if kind == "list":
        tag = "ol" if token["attrs"]["ordered"] else "ul"
        lines = [f"<{tag}>"]
        for item in token["children"]:
            children = [
                child for child in item["children"] if child["type"] != "blank_line"
            ]
            if len(children) != 1 or children[0]["type"] not in (
                "block_text",
                "paragraph",
            ):
                raise UnsupportedMarkdown("list item with nested blocks")
            lines.append(f"  <li>{inline(children[0]['children'])}</li>")
        lines.append(f"</{tag}>")
        return lines
    raise UnsupportedMarkdown(kind)


def parse(changelog: str) -> list[tuple[str, str, list[str]]]:
    """Return (version, date, description lines) for every released section."""
    tokens = mistune.create_markdown(renderer=None)(changelog)
    sections: list[tuple[str, str, list[str]]] = []
    current: list[str] | None = None
    for token in tokens:
        if token["type"] == "heading" and token["attrs"]["level"] <= 2:
            current = None
            match = SECTION.match(plain(token["children"]))
            if match:
                current = []
                sections.append((match["version"], match["date"], current))
        elif token["type"] == "block_html":
            current = None
        elif current is not None:
            try:
                current.extend(block(token))
            except UnsupportedMarkdown as error:
                version = sections[-1][0]
                raise UnsupportedMarkdown(f"[{version}]: unsupported {error}") from None
    return sections


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

    try:
        sections = parse(changelog_path.read_text(encoding="utf-8"))
    except UnsupportedMarkdown as error:
        print(f"{changelog_path}: {error}", file=sys.stderr)
        return 1
    versions = [section[0] for section in sections]
    if version not in versions:
        print(f"{changelog_path}: no section for {version}", file=sys.stderr)
        return 1

    releases = ["  <releases>"]
    for release, date, notes in sections[versions.index(version) :]:
        releases.append(f'    <release version="{release}" date="{date}">')
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
