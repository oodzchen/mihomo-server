import unittest

from scripts.release_notes import extract_notes


class ReleaseNotesTests(unittest.TestCase):
    def test_extracts_only_matching_version_and_preserves_markdown(self):
        changelog = "# Changes\n\n## [Unreleased]\n- future\n\n## [v1.2.3] - 2026-10-25\n\n### 修复\n- 修复更新\n\n## [v1.2.2]\n- old\n"
        self.assertEqual(extract_notes(changelog, "v1.2.3"), "### 修复\n- 修复更新\n")

    def test_missing_empty_duplicate_or_invalid_release_fails(self):
        for changelog, tag in [
            ("## [Unreleased]\n- new", "v1.2.3"),
            ("## [v1.2.3]\n\n### Added\n", "v1.2.3"),
            ("## [v1.2.3]\n- one\n## [v1.2.3]\n- two", "v1.2.3"),
            ("## [../main]\n- bad", "../main"),
        ]:
            with self.subTest(changelog=changelog, tag=tag), self.assertRaises(ValueError):
                extract_notes(changelog, tag)

    def test_last_and_prerelease_sections(self):
        self.assertEqual(extract_notes("## [v1.2.3-beta.1]\n- Preview", "v1.2.3-beta.1"), "- Preview\n")
