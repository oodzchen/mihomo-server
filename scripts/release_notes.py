#!/usr/bin/env python3
"""Use versioned release notes when present, otherwise generate them from Git."""
import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def validate_tag(tag: str) -> None:
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", tag):
        raise ValueError(f"invalid release tag: {tag}")


def extract_notes(changelog: str, tag: str) -> str | None:
    validate_tag(tag)
    sections = list(re.finditer(r"^## \[([^\]\n]+)\][^\n]*$", changelog, re.MULTILINE))
    matches = [(index, section) for index, section in enumerate(sections) if section[1] == tag]
    if not matches:
        return None
    if len(matches) > 1:
        raise ValueError(f"CHANGELOG.md must contain exactly one ## [{tag}] section")
    index, section = matches[0]
    end = sections[index + 1].start() if index + 1 < len(sections) else len(changelog)
    body = changelog[section.end():end].strip()
    if not any(line.strip() and not line.lstrip().startswith("#") for line in body.splitlines()):
        return None
    return body + "\n"


def generate_notes(repository: Path, tag: str) -> str:
    validate_tag(tag)

    def git(*arguments: str) -> str:
        return subprocess.run(["git", *arguments], cwd=repository, check=True,
                              capture_output=True, text=True).stdout.strip()

    # Resolve the tag explicitly so rerunning for an older tag never includes HEAD.
    commit = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
    previous = subprocess.run(
        ["git", "describe", "--tags", "--abbrev=0", "--match", "v[0-9]*", "--exclude", tag, commit],
        cwd=repository, capture_output=True, text=True,
    )
    base = previous.stdout.strip() if previous.returncode == 0 else None
    revision = f"{base}..{commit}" if base else commit
    commits = git("log", "--no-merges", "--reverse", "--format=- %s (%h)", revision)
    introduction = f"自 {base} 以来的提交：" if base else "首次发布，包含以下提交："
    return f"## 更新内容\n\n{introduction}\n\n{commits or '本版本无新增提交。'}\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--changelog", type=Path, default=ROOT / "CHANGELOG.md")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--repo", type=Path, default=ROOT, help="Git repository for automatic release notes")
    args = parser.parse_args()
    try:
        changelog = args.changelog.read_text(encoding="utf-8") if args.changelog.exists() else ""
        notes = extract_notes(changelog, args.tag)
        if notes is None:
            notes = generate_notes(args.repo, args.tag)
    except subprocess.CalledProcessError as error:
        parser.error(error.stderr.strip() or str(error))
    except (ValueError, OSError) as error:
        parser.error(str(error))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(notes, encoding="utf-8")
    else:
        print(notes, end="")


if __name__ == "__main__":
    main()
