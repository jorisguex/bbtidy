import tempfile
import unittest
from pathlib import Path

from scripts.check_upstream_corpus import (
    CompatibilityError,
    discover_metadata_files,
    load_manifest,
    opaque_regions,
    workflow_command_value,
)


class ManifestTests(unittest.TestCase):
    def test_checked_in_manifest_is_valid(self):
        root = Path(__file__).resolve().parents[1]
        manifest = load_manifest(root / "tests" / "upstream-corpus.json")

        self.assertEqual(manifest["schema"], 1)
        self.assertEqual(len(manifest["repositories"]), 2)
        self.assertEqual(len(manifest["layers"]), 4)

    def test_manifest_rejects_floating_revisions(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "manifest.json"
            manifest.write_text(
                """
{
  "schema": 1,
  "repositories": [{
    "name": "example",
    "url": "https://example.com/repository.git",
    "revision": "main",
    "sparse_paths": ["meta"]
  }],
  "layers": [{
    "name": "example",
    "repository": "example",
    "path": "meta",
    "minimum_files": 1
  }]
}
""",
                encoding="utf-8",
            )

            with self.assertRaises(CompatibilityError):
                load_manifest(manifest)


class DiscoveryTests(unittest.TestCase):
    def test_discovers_metadata_without_recipe_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            layer = Path(temporary)
            files = [
                layer / "conf" / "layer.conf",
                layer / "conf" / "distro" / "example.conf",
                layer / "recipes-example" / "example" / "example.bb",
                layer / "recipes-example" / "example" / "example.inc",
                layer / "recipes-example" / "example" / "files" / "template.inc",
                layer / "recipes-example" / "example" / "example" / "runtime.conf",
            ]
            for path in files:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('VALUE = "example"\n', encoding="utf-8")

            discovered = {
                path.relative_to(layer) for path in discover_metadata_files(layer)
            }

            self.assertEqual(
                discovered,
                {
                    Path("conf/layer.conf"),
                    Path("conf/distro/example.conf"),
                    Path("recipes-example/example/example.bb"),
                    Path("recipes-example/example/example.inc"),
                },
            )


class OpaqueRegionTests(unittest.TestCase):
    def test_extracts_functions_and_top_level_python_blocks(self):
        source = (
            "python __anonymous() {\n"
            '    script = """echo "\n'
            "${VALUE}\n"
            '"""\n'
            "}\n"
            "\n"
            "def helper(d):\n"
            "    return d.getVar('VALUE')\n"
            "\n"
            'SUMMARY = "Example"\n'
        )

        self.assertEqual(
            opaque_regions(source),
            [
                (
                    "python __anonymous() {\n"
                    '    script = """echo "\n'
                    "${VALUE}\n"
                    '"""\n'
                    "}\n"
                ),
                "def helper(d):\n    return d.getVar('VALUE')\n\n",
            ],
        )


class WorkflowCommandTests(unittest.TestCase):
    def test_escapes_annotation_control_characters(self):
        self.assertEqual(
            workflow_command_value("100%\r\nfailed"),
            "100%25%0D%0Afailed",
        )


if __name__ == "__main__":
    unittest.main()
