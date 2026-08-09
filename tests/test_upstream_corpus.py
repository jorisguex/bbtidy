import tempfile
import unittest
from pathlib import Path

from scripts.check_upstream_corpus import (
    CompatibilityError,
    compare_semantic_probes,
    discover_metadata_files,
    format_idempotence_command,
    load_manifest,
    lint_command,
    normalize_semantic_values,
    opaque_regions,
    parse_semantic_values,
    verify_syntax_metrics,
    verify_tree_preservation,
    workflow_command_value,
)


class ManifestTests(unittest.TestCase):
    def test_checked_in_manifest_is_valid(self):
        root = Path(__file__).resolve().parents[1]
        manifests = sorted((root / "tests" / "upstream-corpora").glob("*.json"))

        self.assertEqual(len(manifests), 4)
        loaded = [load_manifest(path) for path in manifests]
        self.assertEqual(
            {manifest["id"] for manifest in loaded},
            {
                "community-master",
                "yocto-5.0-scarthgap",
                "yocto-6.0-wrynose",
                "yocto-master",
            },
        )
        self.assertEqual(
            {
                (manifest["yocto_version"], manifest["bitbake_version"])
                for manifest in loaded
                if manifest["tier"] == "supported"
            },
            {("5.0", "2.8"), ("6.0", "2.18")},
        )
        community = next(
            manifest for manifest in loaded if manifest["id"] == "community-master"
        )
        self.assertEqual(community["tier"], "development")
        self.assertEqual(len(community["layers"]), 3)
        self.assertIn("_baseline_metrics", community)
        self.assertEqual(community["_baseline_metrics"]["source"]["files"], 667)
        for manifest in loaded:
            if manifest["tier"] == "supported":
                self.assertEqual(
                    [probe["name"] for probe in manifest["bitbake"]["semantic_probes"]],
                    ["core-image-minimal-metadata"],
                )

    def test_manifest_rejects_floating_revisions(self):
        with tempfile.TemporaryDirectory() as temporary:
            manifest = Path(temporary) / "manifest.json"
            manifest.write_text(
                """
{
  "schema": 1,
  "id": "example",
  "tier": "supported",
  "yocto_version": "1",
  "bitbake_version": "1",
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
  }],
  "syntax_metrics": {
    "minimum_structured_nodes": 1,
    "maximum_unknown_nodes": 0
  },
  "bitbake": {
    "init_repository": "example",
    "template": "meta/conf/templates/default",
    "target": "example",
    "additional_layers": []
  }
}
""",
                encoding="utf-8",
            )

            with self.assertRaises(CompatibilityError):
                load_manifest(manifest)

    def test_development_manifest_accepts_an_explicit_branch_ref(self):
        root = Path(__file__).resolve().parents[1]
        manifest = load_manifest(root / "tests" / "upstream-corpora/yocto-master.json")

        self.assertEqual(manifest["tier"], "development")
        self.assertTrue(
            all(repository["ref"].startswith("refs/heads/")
                for repository in manifest["repositories"])
        )


class SyntaxMetricsTests(unittest.TestCase):
    def test_rejects_structural_coverage_regressions(self):
        source = {
            "version": 1,
            "files": 2,
            "structured_nodes": 10,
            "unknown_nodes": 1,
        }
        formatted = {
            "version": 1,
            "files": 2,
            "structured_nodes": 9,
            "unknown_nodes": 2,
        }

        with self.assertRaises(CompatibilityError):
            verify_syntax_metrics(
                source,
                formatted,
                {"minimum_structured_nodes": 10, "maximum_unknown_nodes": 1},
                2,
            )

    def test_rejects_checked_in_baseline_metric_changes(self):
        source = {
            "version": 1,
            "files": 2,
            "structured_nodes": 10,
            "total_nodes": 12,
            "trivia_nodes": 2,
            "unknown_bytes": 0,
            "unknown_nodes": 0,
        }
        formatted = dict(source)
        baseline = {"source": dict(source), "formatted": dict(formatted)}
        formatted["total_nodes"] = 13

        with self.assertRaisesRegex(CompatibilityError, "baseline"):
            verify_syntax_metrics(
                source,
                formatted,
                {"minimum_structured_nodes": 1, "maximum_unknown_nodes": 1},
                2,
                baseline,
            )


class SemanticProbeTests(unittest.TestCase):
    def test_parses_and_normalizes_selected_bitbake_variables(self):
        values = parse_semantic_values(
            'PN="core-image-minimal"\n'
            'export DEPENDS=" a b "\n'
            'SRC_URI="/tmp/build/downloads/source.tar.gz"\n',
            ["PN", "DEPENDS", "SRC_URI", "MISSING"],
        )

        self.assertEqual(values["PN"], '"core-image-minimal"')
        self.assertEqual(values["MISSING"], None)
        self.assertEqual(
            normalize_semantic_values(values, [Path("/tmp/build")])["SRC_URI"],
            '"<CORPUS>/downloads/source.tar.gz"',
        )

    def test_rejects_semantic_differences(self):
        source = {
            "probe": {"values": {"PN": '"example"', "PV": '"1.0"'}}
        }
        formatted = {
            "probe": {"values": {"PN": '"changed"', "PV": '"1.0"'}}
        }

        with self.assertRaisesRegex(CompatibilityError, "changed variables: PN"):
            compare_semantic_probes(source, formatted)


class TreeVerificationTests(unittest.TestCase):
    def test_allows_only_expected_metadata_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source"
            formatted = Path(temporary) / "formatted"
            (source / "repo" / "meta").mkdir(parents=True)
            (formatted / "repo" / "meta").mkdir(parents=True)
            (source / "repo" / "meta" / "example.bb").write_text(
                'SUMMARY = "before"\n', encoding="utf-8"
            )
            (formatted / "repo" / "meta" / "example.bb").write_text(
                'SUMMARY = "after"\n', encoding="utf-8"
            )
            (source / "repo" / "README").write_text("payload\n", encoding="utf-8")
            (formatted / "repo" / "README").write_text("payload\n", encoding="utf-8")

            self.assertEqual(
                verify_tree_preservation(
                    source, formatted, {"repo/meta/example.bb"}
                ),
                ["repo/meta/example.bb"],
            )

            (formatted / "repo" / "README").write_text(
                "changed\n", encoding="utf-8"
            )
            with self.assertRaisesRegex(CompatibilityError, "allowlist"):
                verify_tree_preservation(source, formatted, {"repo/meta/example.bb"})


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

    def test_uses_current_cli_commands_for_formatting_and_linting(self):
        bbtidy = Path("/tmp/bbtidy")
        inputs = [Path("/tmp/layer")]

        self.assertEqual(
            format_idempotence_command(bbtidy, inputs),
            [bbtidy, "format", "--check", *inputs],
        )
        self.assertEqual(lint_command(bbtidy, inputs), [bbtidy, "check", *inputs])


if __name__ == "__main__":
    unittest.main()
