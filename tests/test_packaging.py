import hashlib
import io
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from scripts.check_release_version import cargo_version, pep440_version, release_tag_from_environment
from scripts.create_release_checksums import create_checksums
from scripts.prepare_release_binaries import (
    SUPPORTED_PLATFORMS,
    prepare_release_binaries,
)
from scripts.release_metadata import load_release_metadata, release_context, wheel_entries
from scripts.smoke_test_package import select_distribution
from scripts.verify_release_artifacts import verify_distributions


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


class ReleaseMetadataTests(unittest.TestCase):
    def test_manifest_is_the_complete_wheel_platform_contract(self):
        metadata = load_release_metadata()
        entries = wheel_entries(metadata)

        self.assertEqual(len(entries), 8)
        self.assertEqual(
            {entry["wheel_platform"] for entry in entries},
            set(SUPPORTED_PLATFORMS),
        )
        self.assertEqual(
            {entry["binary_asset"] for entry in entries},
            set(SUPPORTED_PLATFORMS.values()),
        )
        self.assertEqual(
            sum(entry["container"] is not None for entry in entries),
            5,
        )

    def test_release_context_rejects_a_tag_not_matching_cargo_version(self):
        with self.assertRaisesRegex(ValueError, "does not match Cargo version"):
            release_context("v0.0.0")


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


class ReleaseChecksumTests(unittest.TestCase):
    def test_creates_sha256sum_compatible_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = root / "bbtidy-v1.2.3-linux-x86_64"
            second = root / "bbtidy-v1.2.3-windows-x86_64.exe"
            first.write_bytes(b"linux binary")
            second.write_bytes(b"windows binary")
            manifest = root / "SHA256SUMS"

            create_checksums(root, manifest)

            expected = "".join(
                "{}  {}\n".format(
                    hashlib.sha256(path.read_bytes()).hexdigest(), path.name
                )
                for path in (first, second)
            )
            self.assertEqual(manifest.read_text(encoding="utf-8"), expected)

    def test_rejects_a_directory_without_release_binaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, "no release binaries"):
                create_checksums(root, root / "SHA256SUMS")


class ReleaseArtifactTests(unittest.TestCase):
    def write_distribution_set(self, directory, metadata_version=None):
        version = pep440_version(cargo_version())
        metadata_version = metadata_version or version
        for entry in wheel_entries():
            platform = entry["wheel_platform"]
            executable = "bbtidy.exe" if entry["binary_extension"] else "bbtidy"
            wheel = directory / "bbtidy-{}-py3-none-{}.whl".format(version, platform)
            with zipfile.ZipFile(wheel, "w") as archive:
                archive.writestr(
                    "bbtidy-{}.data/scripts/{}".format(version, executable),
                    b"binary",
                )
                archive.writestr(
                    "bbtidy-{}.dist-info/METADATA".format(version),
                    "Metadata-Version: 2.1\nName: bbtidy\nVersion: {}\n\n".format(
                        metadata_version
                    ),
                )
                archive.writestr(
                    "bbtidy-{}.dist-info/WHEEL".format(version),
                    "Wheel-Version: 1.0\n\n",
                )

        sdist = directory / "bbtidy-{}.tar.gz".format(version)
        payload = "Metadata-Version: 2.1\nName: bbtidy\nVersion: {}\n\n".format(
            metadata_version
        ).encode()
        with tarfile.open(sdist, "w:gz") as archive:
            info = tarfile.TarInfo("bbtidy-{}/PKG-INFO".format(version))
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))

    def test_verifies_complete_wheel_and_source_distribution_set(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory)

            result = verify_distributions(directory)

            version = pep440_version(cargo_version())
            self.assertEqual(result["python_version"], version)
            self.assertEqual(len(result["wheels"]), len(SUPPORTED_PLATFORMS))
            self.assertEqual(result["sdist"].name, "bbtidy-{}.tar.gz".format(version))

    def test_rejects_embedded_package_metadata_version_mismatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory, metadata_version="0.0.0")

            with self.assertRaisesRegex(RuntimeError, "has version"):
                verify_distributions(directory)


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
        self.assertIn("scripts/release_metadata.py", workflow)
        self.assertIn("cargo publish --locked --dry-run", workflow)
        self.assertIn("cargo package --locked --list", workflow)

    def test_github_release_verifies_linux_binaries_and_uploads_checksums(self):
        workflow = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "publish-pypi.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("verify-release-binaries:", workflow)
        self.assertIn("verify-distributions:", workflow)
        self.assertIn("fromJSON(needs.validate.outputs.matrix)", workflow)
        self.assertIn("scripts/verify_release_artifacts.py", workflow)
        self.assertIn("docker/setup-qemu-action@", workflow)
        self.assertIn('test "$actual_version" = "$expected_version"', workflow)
        self.assertIn("SHA256SUMS", workflow)
        self.assertIn("needs: [verify-distributions, verify-release-binaries]", workflow)


if __name__ == "__main__":
    unittest.main()
