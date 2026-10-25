import unittest
from pathlib import Path
import subprocess
import tempfile

from scripts.release_notes import extract_notes, generate_notes


class ReleaseNotesTests(unittest.TestCase):
    def test_extracts_only_matching_version_and_preserves_markdown(self):
        changelog = "# Changes\n\n## [Unreleased]\n- future\n\n## [v1.2.3] - 2026-10-25\n\n### 修复\n- 修复更新\n\n## [v1.2.2]\n- old\n"
        self.assertEqual(extract_notes(changelog, "v1.2.3"), "### 修复\n- 修复更新\n")

    def test_missing_or_empty_notes_allow_automatic_generation(self):
        for changelog in ["", "## [Unreleased]\n- future", "## [v1.2.3]\n\n### Added\n"]:
            with self.subTest(changelog=changelog):
                self.assertIsNone(extract_notes(changelog, "v1.2.3"))

    def test_duplicate_or_invalid_release_fails(self):
        for changelog, tag in [
            ("## [v1.2.3]\n- one\n## [v1.2.3]\n- two", "v1.2.3"),
            ("## [../main]\n- bad", "../main"),
        ]:
            with self.subTest(changelog=changelog, tag=tag), self.assertRaises(ValueError):
                extract_notes(changelog, tag)

    def test_last_and_prerelease_sections(self):
        self.assertEqual(extract_notes("## [v1.2.3-beta.1]\n- Preview", "v1.2.3-beta.1"), "- Preview\n")


class GeneratedReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ms-release-notes-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "release@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        self.git("commit", "--allow-empty", "-m", "feat: initial version")
        self.git("tag", "v1.0.0")

    def git(self, *arguments):
        return subprocess.run(["git", *arguments], cwd=self.repo, check=True,
                              capture_output=True, text=True).stdout.strip()

    def test_first_release_and_no_new_commits(self):
        self.assertIn("feat: initial version", generate_notes(self.repo, "v1.0.0"))
        self.git("tag", "v1.0.1")
        notes = generate_notes(self.repo, "v1.0.1")
        self.assertIn("自 v1.0.0 以来", notes)
        self.assertIn("本版本无新增提交", notes)

    def test_only_target_tag_history_is_included_even_with_newer_head_or_unrelated_tag(self):
        self.git("commit", "--allow-empty", "-m", "fix: 修复更新")
        self.git("tag", "-a", "v1.0.1", "-m", "Release v1.0.1")
        self.git("commit", "--allow-empty", "-m", "feat: future change")
        self.git("tag", "v9.0.0")
        notes = generate_notes(self.repo, "v1.0.1")
        self.assertIn("自 v1.0.0 以来", notes)
        self.assertIn("fix: 修复更新", notes)
        self.assertNotIn("initial version", notes)
        self.assertNotIn("future change", notes)
        self.assertNotIn("v9.0.0", notes)

    def test_missing_tag_fails_instead_of_using_head(self):
        with self.assertRaises(subprocess.CalledProcessError):
            generate_notes(self.repo, "v1.2.3")

    def test_cli_generates_output_without_changelog_and_prefers_curated_notes(self):
        script = Path(__file__).resolve().parents[1] / "release_notes.py"
        changelog, output = self.repo / "CHANGELOG.md", self.repo / "dist/notes.md"
        command = ["python3", str(script), "--repo", str(self.repo), "--tag", "v1.0.0",
                   "--changelog", str(changelog), "--output", str(output)]
        subprocess.run(command, check=True, capture_output=True, text=True)
        self.assertIn("feat: initial version", output.read_text(encoding="utf-8"))
        changelog.write_text("## [v1.0.0]\n### 新增\n- 人工整理的说明\n", encoding="utf-8")
        subprocess.run(command, check=True, capture_output=True, text=True)
        self.assertEqual(output.read_text(encoding="utf-8"), "### 新增\n- 人工整理的说明\n")
        changelog.write_text("## [v1.0.0]\n### 新增\n", encoding="utf-8")
        subprocess.run(command, check=True, capture_output=True, text=True)
        self.assertIn("feat: initial version", output.read_text(encoding="utf-8"))
