"""Regression checks for the temporary fresh-diagnostic publisher."""

import importlib.util
import io
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "fresh_diagnostic", Path(__file__).with_name("fresh-native-diagnostic.py")
)
diagnostic = importlib.util.module_from_spec(spec)
spec.loader.exec_module(diagnostic)


class FreshDiagnosticTests(unittest.TestCase):
    def test_first_compiler_diagnostic_excludes_setup_and_next_error(self):
        result = diagnostic.excerpt(
            io.StringIO(
                "setup output\nerror[E0001]: first failure\n --> source.rs:12\n"
                "help: first fix\nerror: second failure\n"
            )
        )
        self.assertEqual(
            result, "error[E0001]: first failure\n --> source.rs:12\nhelp: first fix"
        )

    def test_test_failure_keeps_test_identity_and_assertion(self):
        result = diagnostic.excerpt(
            io.StringIO(
                "test some_test ... FAILED\nother test passed\n"
                "thread 'some_test' panicked at source.rs:45:\nassertion failed\n"
            )
        )
        self.assertTrue(result.startswith("thread 'some_test' panicked"))
        self.assertIn("assertion failed", result)

    def test_url_userinfo_queries_bearer_and_assignments_are_redacted(self):
        result = diagnostic.sanitize(
            "error: https://user:example-password@private.example/repo?token=abc "
            "Bearer example-value authorization: final-value"
        )
        for private in ["user:", "example-password", "token=abc", "example-value", "final-value"]:
            self.assertNotIn(private, result)

    def test_quoted_assertion_payload_is_redacted(self):
        self.assertEqual(
            diagnostic.sanitize('left: "private-value"'), 'left: "<redacted text>"'
        )

    def test_unclosed_quoted_payload_is_redacted(self):
        self.assertEqual(
            diagnostic.sanitize('left: "private-value'), 'left: "<redacted text>"'
        )

    def test_ansi_removed_and_workflow_command_data_escaped(self):
        result = diagnostic.excerpt(io.StringIO("\x1b[31merror: bad\x1b[0m\n::notice::data%\n"))
        self.assertEqual(
            diagnostic.annotation(result), "error: bad%0A::notice::data%25"
        )

    def test_excerpt_retention_and_lines_are_bounded(self):
        result = diagnostic.excerpt(io.StringIO("error: first\n" + "x" * 2000 + "\n" * 100))
        self.assertLessEqual(len(result), diagnostic.LIMIT)
        self.assertLessEqual(len(result.splitlines()), diagnostic.LINES)

    def test_long_secret_line_continuation_is_discarded(self):
        secret = "private-value-" * 1000
        result = diagnostic.excerpt(io.StringIO("error: first\ntoken=" + secret + "\nhelp: next\n"))
        self.assertNotIn("private-value", result)
        self.assertIn("help: next", result)

    def test_oversized_url_userinfo_is_omitted_entirely(self):
        result = diagnostic.excerpt(
            io.StringIO(
                "error: https://user:" + "private-value-" * 1000 + "@host/repo\n"
            )
        )
        self.assertEqual(result, "[oversized diagnostic line omitted]")

    def test_oversized_quoted_payload_is_omitted_entirely(self):
        result = diagnostic.excerpt(
            io.StringIO('error: payload "' + "private-value-" * 1000 + '"\n')
        )
        self.assertEqual(result, "[oversized diagnostic line omitted]")

    def test_fallback_tail_is_bounded_and_sanitized(self):
        result = diagnostic.excerpt(io.StringIO("noise\n" * 100 + "token=private\n"))
        self.assertLessEqual(len(result.splitlines()), 8)
        self.assertNotIn("private", result)


if __name__ == "__main__":
    unittest.main()
