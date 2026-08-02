import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from scripts.check_release_version import pep440_version, release_tag_from_environment
from scripts.prepare_release_binaries import (
    SUPPORTED_PLATFORMS,
    prepare_release_binaries,
)
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


class ReleaseBinaryTests(unittest.TestCase):
    def test_extracts_one_binary_for_each_wheel_platform(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            wheels = root / "wheels"
            output = root / "release-binaries"
            wheels.mkdir()
            for platform in SUPPORTED_PLATFORMS:
                executable = "bbtidy.exe" if platform == "win_amd64" else "bbtidy"
                wheel = wheels / "bbtidy-1.2.3-py3-none-{}.whl".format(platform)
                with zipfile.ZipFile(wheel, "w") as archive:
                    archive.writestr(
                        "bbtidy-1.2.3.data/scripts/{}".format(executable),
                        ("binary-" + platform).encode(),
                    )

            extracted = prepare_release_binaries(wheels, output, "v1.2.3")

            self.assertEqual(len(extracted), len(SUPPORTED_PLATFORMS))
            self.assertEqual(
                {path.name for path in extracted},
                {
                    "bbtidy-v1.2.3-{}{}".format(
                        suffix, ".exe" if platform == "win_amd64" else ""
                    )
                    for platform, suffix in SUPPORTED_PLATFORMS.items()
                },
            )

    def test_rejects_an_incomplete_wheel_platform_set(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            wheels = root / "wheels"
            wheels.mkdir()
            wheel = wheels / "bbtidy-1.2.3-py3-none-win_amd64.whl"
            with zipfile.ZipFile(wheel, "w") as archive:
                archive.writestr("bbtidy-1.2.3.data/scripts/bbtidy.exe", b"binary")

            with self.assertRaisesRegex(RuntimeError, "missing"):
                prepare_release_binaries(wheels, root / "release-binaries", "v1.2.3")


class ReleaseWorkflowTests(unittest.TestCase):
    def test_crates_publication_is_tag_gated_with_manual_validation(self):
        workflow = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "publish-crates.yml"
        ).read_text(encoding="utf-8")

        self.assertIn('tags: ["v*"]', workflow)
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn(
            "if: ${{ github.event_name == 'push' && github.ref_type == 'tag' }}",
            workflow,
        )
        self.assertIn("needs: package", workflow)
        self.assertIn("cargo publish --locked --dry-run", workflow)


if __name__ == "__main__":
    unittest.main()
