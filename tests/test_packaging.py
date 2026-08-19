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
from scripts.release_metadata import (
    load_release_metadata,
    release_context,
    validate_release_metadata,
    wheel_entries,
)
from scripts.smoke_test_package import onboarding_commands, select_distribution
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
        self.assertEqual(
            next(entry for entry in entries if entry["name"] == "macos-x86_64")[
                "wheel_platform"
            ],
            "macosx_10_12_x86_64",
        )

    def test_release_context_rejects_a_tag_not_matching_cargo_version(self):
        with self.assertRaisesRegex(ValueError, "does not match Cargo version"):
            release_context("v0.0.0")

    def test_manifest_rejects_unsafe_asset_identifiers(self):
        metadata = load_release_metadata()
        metadata["wheel_matrix"][0]["binary_asset"] = "../release"

        with self.assertRaisesRegex(ValueError, "safe identifier"):
            validate_release_metadata(metadata)


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

    def test_package_smoke_runs_the_documented_onboarding_commands(self):
        executable = Path("/venv/bin/bbtidy")
        fixture = Path("/tmp/formatted-fixture.bb")

        self.assertEqual(
            onboarding_commands(executable, fixture),
            [
                [str(executable), "--version"],
                [str(executable), "format", "--check", str(fixture)],
                [
                    str(executable),
                    "check",
                    "--profile",
                    "recommended",
                    str(fixture),
                ],
            ],
        )


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
    def write_distribution_set(
        self, directory, metadata_version=None, composite_manylinux_tags=False
    ):
        version = pep440_version(cargo_version())
        metadata_version = metadata_version or version
        manylinux_compatibility_tags = {
            "manylinux_2_17_x86_64": "manylinux2014_x86_64",
            "manylinux_2_17_aarch64": "manylinux2014_aarch64",
            "manylinux_2_17_armv7l": "manylinux2014_armv7l",
        }
        for entry in wheel_entries():
            platform = entry["wheel_platform"]
            executable = "bbtidy.exe" if entry["binary_extension"] else "bbtidy"
            wheel_platform = platform
            if composite_manylinux_tags and platform in manylinux_compatibility_tags:
                wheel_platform = "{}.{}".format(
                    platform, manylinux_compatibility_tags[platform]
                )
            wheel = directory / "bbtidy-{}-py3-none-{}.whl".format(
                version, wheel_platform
            )
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
                archive.writestr(
                    "bbtidy-{}.dist-info/RECORD".format(version),
                    "",
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

    def test_verifies_composite_manylinux_platform_tags(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory, composite_manylinux_tags=True)

            result = verify_distributions(directory)

            self.assertEqual(len(result["wheels"]), len(SUPPORTED_PLATFORMS))

    def test_rejects_embedded_package_metadata_version_mismatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory, metadata_version="0.0.0")

            with self.assertRaisesRegex(RuntimeError, "has version"):
                verify_distributions(directory)

    def test_rejects_unsafe_wheel_archive_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory)
            wheel = next(directory.glob("bbtidy-*.whl"))
            with zipfile.ZipFile(wheel, "a") as archive:
                archive.writestr("../escape", b"unsafe")

            with self.assertRaisesRegex(RuntimeError, "unsafe archive path"):
                verify_distributions(directory)

    def test_rejects_unsafe_source_distribution_members(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.write_distribution_set(directory)
            version = pep440_version(cargo_version())
            sdist = directory / "bbtidy-{}.tar.gz".format(version)
            with tarfile.open(sdist, "w:gz") as archive:
                unsafe = tarfile.TarInfo("../escape")
                unsafe.size = 6
                archive.addfile(unsafe, io.BytesIO(b"unsafe"))

            with self.assertRaisesRegex(RuntimeError, "unsafe path"):
                verify_distributions(directory)


class ToolchainTests(unittest.TestCase):
    def test_ci_toolchain_pins_clippy_and_rustfmt(self):
        toolchain = (Path(__file__).resolve().parents[1] / "rust-toolchain.toml").read_text(
            encoding="utf-8"
        )

        self.assertIn('channel = "1.91.1"', toolchain)
        self.assertIn('components = ["clippy", "rustfmt"]', toolchain)


class ReleaseWorkflowTests(unittest.TestCase):
    def test_crates_publisher_is_reusable_and_release_is_tag_gated(self):
        workflow = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "publish-crates.yml"
        ).read_text(encoding="utf-8")
        release = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "release.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("on:\n  workflow_call:", workflow)
        self.assertNotIn("workflow_dispatch:", workflow)
        self.assertIn('tags: ["v*"]', release)
        self.assertIn("uses: ./.github/workflows/release-gate.yml", release)
        self.assertIn("needs: [metadata, release-gate]", release)
        self.assertIn("scripts/release_metadata.py", workflow)
        self.assertIn("actions/setup-python@", workflow)
        self.assertIn('python-version: "3.12"', workflow)
        self.assertIn("cargo publish --locked --dry-run", workflow)
        self.assertIn("cargo package --locked --list", workflow)

    def test_github_release_verifies_linux_binaries_and_uploads_checksums(self):
        workflow = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "release.yml"
        ).read_text(encoding="utf-8")

        pypi = (
            Path(__file__).resolve().parents[1]
            / ".github"
            / "workflows"
            / "publish-pypi.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("verify-release-binaries:", pypi)
        self.assertIn("verify-distributions:", pypi)
        self.assertIn("fromJSON(needs.validate.outputs.matrix)", pypi)
        self.assertIn("scripts/verify_release_artifacts.py", pypi)
        self.assertGreaterEqual(pypi.count("actions/setup-python@"), 5)
        self.assertIn('python-version: "3.12"', workflow)
        self.assertIn("docker/setup-qemu-action@", pypi)
        self.assertIn('test "$actual_version" = "$expected_version"', pypi)
        self.assertIn("SHA256SUMS", workflow)
        self.assertIn("release-evidence.tar.gz", workflow)
        self.assertIn("needs: [metadata, release-gate, publish-python]", workflow)

    def test_release_workflows_run_packaging_and_workflow_validation(self):
        root = Path(__file__).resolve().parents[1] / ".github" / "workflows"
        crates = (root / "publish-crates.yml").read_text(encoding="utf-8")
        python_package = (root / "python-package.yml").read_text(encoding="utf-8")
        pypi = (root / "publish-pypi.yml").read_text(encoding="utf-8")

        self.assertIn('python3 -m unittest discover -s tests -p "test_*.py"', crates)
        self.assertIn("python3 scripts/check_workflows.py", crates)
        for workflow in (python_package, pypi):
            self.assertIn(
                "scripts/smoke_test_package.py --kind wheel",
                workflow,
            )
            self.assertIn(
                "scripts/smoke_test_package.py --kind sdist",
                workflow,
            )


if __name__ == "__main__":
    unittest.main()
