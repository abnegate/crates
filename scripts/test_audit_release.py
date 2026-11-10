"""Tests for audit_release.py, each run against a release-plz.toml written for it."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / 'audit_release.py'
CONFIGURATION = 'release-plz.toml'
TYPES = 'feat|fix|perf|refactor|docs|test|style|ci|build|revert'
PATTERN = (
    rf'^(({TYPES})(\([^()]+\))?!?:|[a-z]+(\([^()]+\))?!:'
    r'|chore: update Cargo\.(toml|lock) dependencies)'
)
SKIP = r'^chore(\([^()]+\))?: release( v[0-9][^ \n]*)?( \([^ \n]+\))?(\n|$)'
FIXED = "{ message = '^fix', group = 'fixed' }"
OTHER = "{ message = '.*', group = 'other' }"
LINKED = '([#79](https://github.com/abnegate/crates/pull/79))'
PASSED = 0
PROBLEMS = 1
ERROR = 2


def skip(pattern: str = SKIP) -> str:
    return f"{{ message = '{pattern}', skip = true }}"


def changelog(*parsers: str) -> str:
    return '\n'.join(['[changelog]', 'commit_parsers = [', *(f'    {parser},' for parser in parsers), ']', ''])


def workspace(*settings: str, parsers: tuple[str, ...] | None = None) -> str:
    return '\n'.join(
        [
            '[workspace]',
            'changelog_update = true',
            *settings,
            '',
            changelog(*(skip(), FIXED, OTHER) if parsers is None else parsers),
        ]
    )


def always(value: str = 'false') -> str:
    return f'release_always = {value}'


def commits(pattern: str = PATTERN) -> str:
    return f"release_commits = '{pattern}'"


class AuditTest(unittest.TestCase):
    def fixture(self, configuration: str | None) -> Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        script = root / 'scripts' / SCRIPT.name
        script.parent.mkdir()
        shutil.copyfile(SCRIPT, script)
        if configuration is not None:
            (root / CONFIGURATION).write_text(configuration, encoding='utf-8')
        return script

    def audit(self, configuration: str | None) -> subprocess.CompletedProcess[str]:
        return self.python(str(self.fixture(configuration)))

    def audit_as_python(self, version: tuple[object, ...], configuration: str) -> subprocess.CompletedProcess[str]:
        program = textwrap.dedent(f"""
            import runpy
            import sys

            sys.version_info = {version!r}
            runpy.run_path({str(self.fixture(configuration))!r}, run_name='__main__')
        """)
        return self.python('-c', program)

    def python(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, *arguments], capture_output=True, text=True, check=False)

    def problems(self, configuration: str) -> list[str]:
        result = self.audit(configuration)
        self.assertEqual(result.returncode, PROBLEMS, result.stderr)
        self.assertIn(f'problem(s) above with what {CONFIGURATION} releases and when', result.stderr)
        prefix = f'{CONFIGURATION}: '
        lines = result.stdout.splitlines()
        self.assertTrue(all(line.startswith(prefix) for line in lines), result.stdout)
        return [line.removeprefix(prefix) for line in lines]

    def refused(self, pattern: str, rest: str) -> None:
        problems = self.problems(workspace(always(), commits(pattern)))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_commits` leaves the syntax', problems[0])
        self.assertIn(f'read alike at `{rest}`', problems[0])
        self.assertIn('so it cannot be judged', problems[0])

    def parsed(self, *parsers: str) -> list[str]:
        return self.problems(workspace(always(), commits(), parsers=parsers))

    def test_a_configuration_that_publishes_only_from_the_release_pull_request_passes(self) -> None:
        result = self.audit(workspace(always(), commits()))

        self.assertEqual((result.returncode, result.stdout, result.stderr), (PASSED, '', ''))

    def test_the_configuration_this_repository_ships_passes(self) -> None:
        result = self.python(str(SCRIPT))

        self.assertEqual((result.returncode, result.stdout, result.stderr), (PASSED, '', ''))

    def test_publishing_on_every_push_is_a_problem(self) -> None:
        problems = self.problems(workspace(always('true'), commits()))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_always` must be false in [workspace]', problems[0])

    def test_leaving_release_always_to_its_default_is_a_problem(self) -> None:
        problems = self.problems(workspace(commits()))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_always` must be false in [workspace]', problems[0])

    def test_a_release_always_that_is_not_a_boolean_is_a_problem(self) -> None:
        problems = self.problems(workspace(always('"false"'), commits()))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_always` must be false in [workspace]', problems[0])

    def test_a_missing_release_commits_is_a_problem(self) -> None:
        problems = self.problems(workspace(always()))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_commits` must be a pattern in [workspace]', problems[0])

    def test_settings_outside_the_workspace_table_do_not_count(self) -> None:
        problems = self.problems('\n'.join([always(), commits(), '', workspace()]))

        self.assertEqual(len(problems), 2, problems)
        self.assertIn('`release_always` must be false in [workspace]', problems[0])
        self.assertIn('`release_commits` must be a pattern in [workspace]', problems[1])

    def test_a_pattern_a_release_commit_matches_names_each_commit_it_lets_through(self) -> None:
        problems = self.problems(workspace(always(), commits(PATTERN.replace(TYPES, f'{TYPES}|chore'))))

        self.assertEqual(
            problems,
            [
                f'`release_commits` lets {subject} prepare a release, which it must hold back'
                for subject in (
                    '`chore: release`',
                    '`chore: release (#79)`',
                    '`chore: release v0.1.13 (#62)`',
                    '`chore(abnegate-vcs): release v0.1.13 (#77)`',
                    '`chore: release (#79)` followed by a body',
                    '`chore(deps): prune the workspace dependencies no crate takes`',
                )
            ],
        )

    def test_a_pattern_that_reads_a_release_commit_body_is_a_problem(self) -> None:
        problems = self.problems(workspace(always(), commits(PATTERN.removeprefix('^'))))

        self.assertEqual(
            problems,
            ['`release_commits` lets `chore: release (#79)` followed by a body prepare a release, which it must hold back'],
        )

    def test_a_pattern_every_type_matches_is_a_problem(self) -> None:
        problems = self.problems(workspace(always(), commits(r'^[a-z]+(\([^()]+\))?!?:')))

        for subject in (
            'chore: release (#79)',
            'chore(abnegate-vcs): release v0.1.13 (#77)',
            'chore(deps): prune the workspace dependencies no crate takes',
        ):
            with self.subTest(subject=subject):
                self.assertIn(f'`release_commits` lets `{subject}` prepare a release, which it must hold back', problems)
        self.assertNotIn('Merge pull request', '\n'.join(problems))

    def test_a_pattern_a_merge_commit_matches_is_a_problem(self) -> None:
        problems = self.problems(workspace(always(), commits(f'{PATTERN}|^Merge ')))

        self.assertEqual(
            problems,
            [
                '`release_commits` lets `Merge pull request #11 from abnegate/release-plz-2026-10-01T09-54-53Z` '
                'prepare a release, which it must hold back'
            ],
        )

    def test_a_pattern_that_holds_back_a_test_style_or_ci_commit_is_a_problem(self) -> None:
        problems = self.problems(
            workspace(always(), commits(PATTERN.replace(TYPES, 'feat|fix|perf|refactor|docs|build|revert')))
        )

        self.assertEqual(
            problems,
            [
                f'`release_commits` holds back `{subject}`, which must prepare a release'
                for subject in (
                    'test(secret): read the plaintext report in a process no other test shares (#47)',
                    'test: cover the retry budget',
                    'style(agent): space the rmcp dev-dependency like its siblings',
                    'style: one import per statement',
                    'ci: install ripgrep so the agent test runs on linux',
                )
            ],
        )

    def test_a_pattern_that_holds_back_a_fix_or_a_breaking_change_is_a_problem(self) -> None:
        problems = self.problems(workspace(always(), commits(r'^feat(\([^()]+\))?:')))

        for subject in (
            'fix(vcs): refuse a configured_origin path that is not a regular file (#45)',
            'feat!: drop the constructor nothing calls',
            'fix(deps)!: move rmcp past its advisories and drop the rusqlite feature (#78)',
            'chore(deps)!: raise the minimum supported rust version',
            'chore: update Cargo.toml dependencies',
        ):
            with self.subTest(subject=subject):
                self.assertIn(f'`release_commits` holds back `{subject}`, which must prepare a release', problems)
        self.assertNotIn('must hold back', '\n'.join(problems))

    def test_syntax_only_one_engine_reads_is_refused(self) -> None:
        for pattern, rest in (
            (r'^(?!chore)[a-z]+:', '(?!chore)[a-z]+:'),
            (r'^(?=feat|fix)', '(?=feat|fix)'),
            (r'(?<=^)feat:', '(?<=^)feat:'),
            (r'^(feat|fix)\1:', r'\1:'),
            (r'(?m)^(feat|fix):', '(?m)^(feat|fix):'),
            (r'(?i)^(feat|fix):', '(?i)^(feat|fix):'),
            (r'^(?P<kind>feat|fix):', '(?P<kind>feat|fix):'),
            (r'^(?>feat|fix):', '(?>feat|fix):'),
            (r'^(feat|fix)\b', r'\b'),
            (r'^\w+!:', r'\w+!:'),
            (r'^(feat|fix):.*$', '$'),
            (r'^(feat|fix):[^\n]+', r'[^\n]+'),
            (r'^(feat|fix):.*\n', r'\n'),
            (r'^(feat|fix)\d', r'\d'),
            (r'^(feat|fix)*+:', '+:'),
            (r'^[[:lower:]]+!:', '[[:lower:]]+!:'),
            (r'^[a-z&&[^c]]+!:', '[a-z&&[^c]]+!:'),
            (r'^(feat|fix){,2}:', '{,2}:'),
        ):
            with self.subTest(pattern=pattern):
                self.refused(pattern, rest)

    def test_a_pattern_neither_engine_can_compile_is_a_problem(self) -> None:
        problems = self.problems(workspace(always(), commits('^(feat|fix:')))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`release_commits` is not a pattern', problems[0])

    def test_a_missing_changelog_table_is_a_problem(self) -> None:
        problems = self.problems('\n'.join(['[workspace]', always(), commits(), '']))

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`commit_parsers` must be a list of tables in [changelog]', problems[0])

    def test_an_empty_parser_list_is_a_problem(self) -> None:
        problems = self.parsed()

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('`commit_parsers` must be a list of tables in [changelog]', problems[0])

    def test_parsers_without_the_one_that_skips_release_commits_are_a_problem(self) -> None:
        problems = self.parsed(FIXED, OTHER)

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('the first entry of `commit_parsers` must have `skip = true` and a `message`', problems[0])

    def test_a_skip_listed_after_the_catch_all_is_a_problem(self) -> None:
        problems = self.parsed(FIXED, OTHER, skip())

        self.assertEqual(len(problems), 2, problems)
        self.assertIn('the first entry of `commit_parsers` must have `skip = true`', problems[0])
        self.assertIn('the last entry of `commit_parsers` must be the `.*` catch-all', problems[1])

    def test_parsers_without_the_catch_all_are_a_problem(self) -> None:
        problems = self.parsed(skip(), FIXED)

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('the last entry of `commit_parsers` must be the `.*` catch-all and must not skip', problems[0])

    def test_a_catch_all_that_skips_is_a_problem(self) -> None:
        problems = self.parsed(skip(), FIXED, "{ message = '.*', skip = true }")

        self.assertEqual(len(problems), 1, problems)
        self.assertIn('the last entry of `commit_parsers` must be the `.*` catch-all and must not skip', problems[0])

    def test_a_skip_that_misses_a_linked_release_commit_is_a_problem(self) -> None:
        problems = self.parsed(skip(r'^chore(\([^()]+\))?: release( v[0-9][^ \n]*)?( \(#[0-9]+\))?(\n|$)'), OTHER)

        self.assertEqual(
            problems,
            [
                f'the first `commit_parsers` pattern does not skip {subject}, a release commit'
                for subject in (
                    f'`chore: release {LINKED}`',
                    '`chore: release v0.1.13 ([#62](https://github.com/abnegate/crates/pull/62))`',
                    '`chore(abnegate-vcs): release v0.1.13 ([#77](https://github.com/abnegate/crates/pull/77))`',
                    f'`chore: release {LINKED}` followed by a body',
                )
            ],
        )

    def test_a_skip_that_misses_a_release_commit_with_a_body_is_a_problem(self) -> None:
        problems = self.parsed(skip(SKIP.removesuffix(r'(\n|$)') + '$'), OTHER)

        self.assertEqual(
            problems,
            [
                f'the first `commit_parsers` pattern does not skip {subject} followed by a body, a release commit'
                for subject in ('`chore: release (#79)`', f'`chore: release {LINKED}`')
            ],
        )

    def test_a_skip_that_takes_another_commit_is_a_problem(self) -> None:
        problems = self.parsed(skip('^chore'), OTHER)

        self.assertEqual(
            problems,
            [
                f'the first `commit_parsers` pattern skips `{subject}`, which is not a release commit'
                for subject in (
                    'chore(deps)!: raise the minimum supported rust version',
                    'chore: update Cargo.toml dependencies',
                    'chore: update Cargo.lock dependencies',
                    'chore(deps): prune the workspace dependencies no crate takes',
                )
            ],
        )

    def test_a_skip_in_syntax_only_one_engine_reads_is_refused(self) -> None:
        for pattern, rest in (
            (r'^chore: release( v\d\S*)?(\n|$)', r'\d\S*)?(\n|$)'),
            (r'^chore: release( \(\S+\))?(\n|$)', r'\S+\))?(\n|$)'),
            (r'(?m)^chore: release$', '(?m)^chore: release$'),
            (r'^chore: release\b', r'\b'),
            (r'^chore: release\Z', r'\Z'),
            (r'^chore: release$?', '?'),
        ):
            with self.subTest(pattern=pattern):
                problems = self.parsed(skip(pattern), OTHER)

                self.assertEqual(len(problems), 1, problems)
                self.assertIn('the first `commit_parsers` pattern leaves the syntax', problems[0])
                self.assertIn(f'read alike at `{rest}`', problems[0])

    def test_a_missing_configuration_fails(self) -> None:
        result = self.audit(None)

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn(f'cannot read {CONFIGURATION}', result.stderr)

    def test_a_configuration_that_is_not_toml_fails(self) -> None:
        result = self.audit('[workspace\n')

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn(f'{CONFIGURATION} is not TOML', result.stderr)

    def test_a_python_older_than_the_minimum_is_refused_before_anything_is_audited(self) -> None:
        result = self.audit_as_python((3, 10, 14, 'final', 0), workspace(always('true')))

        self.assertEqual((result.returncode, result.stdout), (ERROR, ''), result.stderr)
        self.assertIn('needs Python 3.11 or later, but this is Python 3.10.14', result.stderr)


if __name__ == '__main__':
    unittest.main()
