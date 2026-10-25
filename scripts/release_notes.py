#!/usr/bin/env python3
"""Extract one tagged version's release body from CHANGELOG.md."""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent


def extract_notes(changelog: str, tag: str) -> str:
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", tag):
        raise ValueError(f"invalid release tag: {tag}")
    sections = list(re.finditer(r"^## \[([^\]\n]+)\][^\n]*$", changelog, re.MULTILINE))
    matches = [(index, section) for index, section in enumerate(sections) if section[1] == tag]
    if len(matches) != 1:
        raise ValueError(f"CHANGELOG.md must contain exactly one ## [{tag}] section")
    index, section = matches[0]
    end = sections[index + 1].start() if index + 1 < len(sections) else len(changelog)
    body = changelog[section.end():end].strip()
    if not any(line.strip() and not line.lstrip().startswith("#") for line in body.splitlines()):
        raise ValueError(f"release notes for {tag} are empty")
    return body + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--changelog", type=Path, default=ROOT / "CHANGELOG.md")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        notes = extract_notes(args.changelog.read_text(encoding="utf-8"), args.tag)
    except (ValueError, OSError) as error:
        parser.error(str(error))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(notes, encoding="utf-8")
    else:
        print(notes, end="")


if __name__ == "__main__":
    main()
