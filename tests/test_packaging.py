import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.check_release_version import pep440_version, release_tag_from_environment
from scripts.smoke_test_package import select_distribution


class VersionTests(unittest.TestCase):
    def test_semver_prereleases_map_to_pep440(self):
        self.assertEqual(pep440_version("1.2.3"), "1.2.3")
        self.assertEqual(pep440_version("1.2.3-alpha.4"), "1.2.3a4")
        self.assertEqual(pep440_version("1.2.3-beta.5"), "1.2.3b5")
        self.assertEqual(pep440_version("1.2.3-rc.6"), "1.2.3rc6")

    def test_ambiguous_versions_are_rejected(self):
        with self.assertRaises(ValueError):
            pep440_version("1.2.3-dev.1")

    def test_github_tag_is_only_read_in_tag_context(self):
        with mock.patch.dict(
            "os.environ",
            {"GITHUB_REF_TYPE": "branch", "GITHUB_REF_NAME": "main"},
            clear=True,
        ):
            self.assertIsNone(release_tag_from_environment())
        with mock.patch.dict(
            "os.environ",
            {"GITHUB_REF_TYPE": "tag", "GITHUB_REF_NAME": "v1.2.3"},
            clear=True,
        ):
            self.assertEqual(release_tag_from_environment(), "v1.2.3")


class DistributionSelectionTests(unittest.TestCase):
    def test_selects_exactly_one_requested_distribution(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            wheel = directory / "bbtidy.whl"
            sdist = directory / "bbtidy.tar.gz"
            wheel.touch()
            sdist.touch()

            self.assertEqual(select_distribution(directory, "wheel"), wheel.resolve())
            self.assertEqual(select_distribution(directory, "sdist"), sdist.resolve())

    def test_rejects_ambiguous_distributions(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "first.whl").touch()
            (directory / "second.whl").touch()

            with self.assertRaises(RuntimeError):
                select_distribution(directory, "wheel")


if __name__ == "__main__":
    unittest.main()
