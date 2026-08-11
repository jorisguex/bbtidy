import unittest

from scripts.syntax_inventory import classify, normalize_signature


class SyntaxInventoryTests(unittest.TestCase):
    def test_normalization_keeps_provider_shape_and_removes_literals(self):
        signature = normalize_signature(
            'PREFERRED_PROVIDER_virtual/kernel ?= "${KERNEL_PROVIDER}"'
        )
        self.assertEqual(
            signature,
            'PREFERRED_PROVIDER_virtual/<provider> ?= "STRING"',
        )

    def test_only_reviewed_provider_group_is_classified(self):
        self.assertEqual(
            classify('PREFERRED_PROVIDER_virtual/<provider> = "STRING"'),
            "valid_bitbake_syntax",
        )
        self.assertEqual(
            classify('LICENSE:<scope>/<component> = "STRING"'),
            "valid_bitbake_syntax",
        )
        self.assertEqual(
            classify("unreviewed top-level construct"),
            "uncertain_requires_bitbake_probe",
        )


if __name__ == "__main__":
    unittest.main()
