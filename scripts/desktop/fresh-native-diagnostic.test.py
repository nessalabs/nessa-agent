"""Regression checks for the temporary fresh-diagnostic publisher."""

import importlib.util
import io
import json
import tempfile
import subprocess
import sys
import os
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

    def test_test_failure_keeps_source_and_assertion_while_redacting_thread_name(self):
        result = diagnostic.excerpt(
            io.StringIO(
                "test some_test ... FAILED\nother test passed\n"
                "thread 'some_test' panicked at source.rs:45:\nassertion failed\n"
            )
        )
        self.assertTrue(result.startswith("thread <redacted> panicked"))
        self.assertNotIn("some_test", result)
        self.assertIn("source.rs:45", result)
        self.assertIn("assertion failed", result)

    def test_prefixed_server_panic_keeps_first_assertion_before_tail_noise(self):
        result = diagnostic.excerpt(io.StringIO(
            "[nessa-server nessa_server] thread 'case' panicked at source.rs:45:\n"
            "[nessa-server nessa_server] assertion failed: identity\n" + "tail\n" * 100
        ))
        self.assertTrue(result.startswith("thread <redacted> panicked"))
        self.assertNotIn("case", result)
        self.assertIn("source.rs:45", result)
        self.assertIn("assertion failed: identity", result)

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

    def test_single_quoted_objects_are_redacted_at_both_publication_sinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "fresh.log"
            summary = root / "summary.md"
            log.write_text("error: {'api-key': 'synthetic-private-marker'}\n")
            env = os.environ.copy()
            env["GITHUB_STEP_SUMMARY"] = str(summary)
            result = subprocess.run([sys.executable, diagnostic.__file__, str(log)],
                                    env=env, text=True, capture_output=True, check=True)
            for sink in [result.stdout, summary.read_text()]:
                self.assertNotIn("synthetic-private-marker", sink)
                self.assertIn("redacted text", sink)

    def test_unclosed_single_quoted_payload_is_redacted(self):
        self.assertNotIn("private-marker", diagnostic.sanitize("error: 'private-marker"))

    def test_escaped_quotes_and_trailing_escape_do_not_expose_payload(self):
        cases = [
            "error: 'private-marker\\'still-private-marker' after",
            'error: "private-marker\\"still-private-marker" after',
            "error: 'private-marker\\",
            'error: "private-marker\\',
        ]
        for value in cases:
            with self.subTest(value=value):
                result = diagnostic.sanitize(value)
                self.assertNotIn("private-marker", result)
                self.assertIn("redacted text", result)

    def test_quote_context_cannot_reopen_a_discarded_tail_at_either_sink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, prefix in enumerate(["can't read ", "doesn't match ", "'", '"', "`", "\\'", 'path\\"']):
                log = root / "fresh.log"
                summary = root / (str(index) + ".md")
                log.write_text("error: " + prefix + "'synthetic-private-marker'\n")
                env = os.environ.copy()
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                result = subprocess.run([sys.executable, diagnostic.__file__, str(log)],
                                        env=env, text=True, capture_output=True, check=True)
                for sink in [result.stdout, summary.read_text()]:
                    self.assertNotIn("synthetic-private-marker", sink)
                    self.assertIn("redacted text", sink)

    def test_http_scheme_casing_redacts_userinfo_at_both_sinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, scheme in enumerate(["https", "HTTPS", "hTtP", "HtTpS"]):
                log = root / "fresh.log"
                summary = root / (str(index) + ".md")
                log.write_text("error: " + scheme + "://alice:synthetic-private-marker@example.invalid/repo\n")
                env = os.environ.copy()
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                result = subprocess.run([sys.executable, diagnostic.__file__, str(log)],
                                        env=env, text=True, capture_output=True, check=True)
                for sink in [result.stdout, summary.read_text()]:
                    self.assertNotIn("synthetic-private-marker", sink)
                    self.assertIn("redacted URL", sink)

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


class FreshBrowserResultTests(unittest.TestCase):
    def fixture(self, root):
        record = {"repository": "nessalabs/nessa-agent", "workflowCommit": "a" * 40,
                  "source": "b" * 40, "base": None, "tree": "c" * 40, "state": "ready"}
        (root / "provenance.json").write_text(json.dumps(record))
        return record

    def test_browser_provenance_and_summary_are_published(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "pr-summary.md").write_text("Chromium passed\nWebKit passed\n")
            result = diagnostic.browser_result(root)
            self.assertIn("source: " + "b" * 40, result)
            self.assertIn("WebKit passed", result)
            self.assertIn("state: ready", result)

    def test_browser_summary_is_bounded_and_redacted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "pr-summary.md").write_text("token=private-value\n" + "line\n" * 10000)
            result = diagnostic.browser_result(root)
            self.assertNotIn("private-value", result)
            self.assertLess(len(result), diagnostic.LIMIT + 500)
            self.assertLess(len(result.splitlines()), diagnostic.LINES + 10)

    def test_browser_refuses_untrusted_provenance_values(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for field, value in [("source", "private-value"), ("repository", "foreign/repo"),
                                 ("state", "failed"), ("tree", None), ("base", "invalid")]:
                record = self.fixture(root)
                record[field] = value
                (root / "provenance.json").write_text(json.dumps(record))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    diagnostic.browser_result(root)

    def test_missing_browser_summary_is_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            self.assertIn("summary was not produced", diagnostic.browser_result(root))

    def test_browser_failure_is_selected_before_tail_noise(self):
        result = diagnostic.excerpt(io.StringIO("setup\nFAIL scripted scenario\nassertion failed\n" + "tail\n" * 100))
        self.assertTrue(result.startswith("FAIL scripted scenario"))
        self.assertIn("assertion failed", result)


if __name__ == "__main__":
    unittest.main()
