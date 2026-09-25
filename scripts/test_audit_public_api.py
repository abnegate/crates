"""Tests for audit_public_api.py, each run against a crate written for it."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / 'audit_public_api.py'
MANIFEST = """
    [package]
    name = "fixture"
    version = "0.1.0"
    edition = "2024"

    [dependencies]
    serde = "1"
"""
PASSED = 0
OFFENDERS = 1
ERROR = 2


class AuditTest(unittest.TestCase):
    def fixture(self, library: str) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        script = root / 'scripts' / SCRIPT.name
        script.parent.mkdir()
        shutil.copyfile(SCRIPT, script)
        crate = root / 'crates' / 'fixture'
        (crate / 'src').mkdir(parents=True)
        (crate / 'Cargo.toml').write_text(textwrap.dedent(MANIFEST).lstrip(), encoding='utf-8')
        (crate / 'src' / 'lib.rs').write_text(textwrap.dedent(library).lstrip(), encoding='utf-8')
        return script

    def audit(self, library: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(self.fixture(library))],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_a_type_that_can_gain_a_field_passes(self) -> None:
        result = self.audit("""
            #[non_exhaustive]
            pub struct Settings {
                pub name: String,
            }

            #[non_exhaustive]
            pub enum Shape {
                #[non_exhaustive]
                Circle { radius: f64 },
                Point,
            }
        """)

        self.assertEqual((result.returncode, result.stdout), (PASSED, ''), result.stderr)

    def test_a_struct_variant_that_cannot_gain_a_field_is_printed(self) -> None:
        result = self.audit("""
            #[non_exhaustive]
            pub enum Shape {
                Circle { radius: f64 },
            }
        """)

        self.assertEqual(result.returncode, OFFENDERS, result.stderr)
        self.assertEqual(result.stdout, 'crates/fixture/src/lib.rs:3: Shape::Circle\n')

    def test_a_re_export_of_a_path_outside_the_crate_is_skipped(self) -> None:
        result = self.audit("""
            pub use Option::Some;
            pub use Result::Ok as Fine;
            pub use String as Text;
            pub use serde::Serialize;
            pub use std::collections::HashMap;
        """)

        self.assertEqual((result.returncode, result.stdout, result.stderr), (PASSED, '', ''))

    def test_a_re_export_that_enters_the_crate_and_finds_nothing_fails(self) -> None:
        result = self.audit("""
            mod inner {}

            pub use inner::Missing;
        """)

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn('cannot resolve the `pub use` at crates/fixture/src/lib.rs:3', result.stderr)

    def test_a_re_export_of_a_name_a_glob_brings_in_unresolved_fails(self) -> None:
        result = self.audit("""
            mod inner {
                pub use crate::missing::Shape;
            }

            use inner::*;

            pub use Shape::Circle;
        """)

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn('cannot resolve the `pub use` at crates/fixture/src/lib.rs:7', result.stderr)

    def test_a_module_without_a_file_fails(self) -> None:
        result = self.audit("""
            pub mod gone;
        """)

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn('no file for `mod gone;`', result.stderr)


if __name__ == '__main__':
    unittest.main()
