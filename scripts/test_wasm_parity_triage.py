import unittest

from scripts.wasm_parity_triage import classify


class WasmParityTriageTests(unittest.TestCase):
    def test_yasgui_only_stamp_change_is_classified_as_stamp(self):
        old = b'<script>const BUILD_STAMP = "Built dev.";</script>'
        new = b'<script>const BUILD_STAMP = "Built 0.3.2.";</script>'

        finding = classify("docs/yasgui.html", old, new, "0.3.2")

        self.assertEqual(finding.cause, "STAMP")
        self.assertIn('rebuilt as "0.3.2"', finding.detail)


if __name__ == "__main__":
    unittest.main()
