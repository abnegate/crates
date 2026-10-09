"""Tests for affected_crates.py, against modelled metadata and a real workspace."""

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / 'affected_crates.py'
SPECIFICATION = importlib.util.spec_from_file_location('affected_crates', SCRIPT)
affected_crates = importlib.util.module_from_spec(SPECIFICATION)
sys.modules[SPECIFICATION.name] = affected_crates
SPECIFICATION.loader.exec_module(affected_crates)

SELECTED = 0
ERROR = 2
ROOT = '/workspace'
GRAPH = {
    'abnegate-secret': [],
    'abnegate-http': [],
    'abnegate-vision': [],
    'abnegate-exec': [('abnegate-secret', None, False)],
    'abnegate-llm': [('abnegate-http', None, False), ('abnegate-secret', None, False)],
    'abnegate-search': [('abnegate-secret', None, False)],
    'abnegate-agent-cli': [('abnegate-exec', None, False), ('abnegate-llm', None, False)],
    'abnegate-agent': [('abnegate-agent-cli', None, True), ('abnegate-llm', 'dev', False)],
    'abnegate-comfy': [('abnegate-vision', 'build', False)],
}
EVERY = sorted(GRAPH)


def metadata(graph: dict[str, list[tuple[str, str | None, bool]]]) -> dict[str, object]:
    def identifier(name: str) -> str:
        return f'path+file://{ROOT}/crates/{name}#0.1.0'

    def dependency(name: str, kind: str | None, optional: bool) -> dict[str, object]:
        return {'name': name, 'kind': kind, 'optional': optional, 'path': f'{ROOT}/crates/{name}'}

    packages = [
        {
            'id': identifier(name),
            'name': name,
            'manifest_path': f'{ROOT}/crates/{name}/Cargo.toml',
            'dependencies': [
                {'name': 'serde', 'kind': None, 'optional': False},
                *(dependency(*edge) for edge in edges),
            ],
        }
        for name, edges in graph.items()
    ]
    packages.append({
        'id': 'registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0',
        'name': 'serde',
        'manifest_path': '/registry/serde-1.0.0/Cargo.toml',
        'dependencies': [],
    })
    return {
        'workspace_root': ROOT,
        'workspace_members': [identifier(name) for name in graph],
        'packages': packages,
    }


def workspace() -> affected_crates.Workspace:
    return affected_crates.Workspace.from_metadata(metadata(GRAPH))


class WorkspaceTest(unittest.TestCase):
    def test_only_workspace_members_are_crates(self) -> None:
        self.assertEqual(sorted(workspace().names), EVERY)

    def test_a_crate_is_found_by_its_directory(self) -> None:
        self.assertEqual(workspace().owner('crates/abnegate-agent-cli/src/lib.rs').name, 'abnegate-agent-cli')

    def test_a_crate_whose_name_prefixes_another_does_not_own_it(self) -> None:
        self.assertEqual(workspace().owner('crates/abnegate-agent/src/lib.rs').name, 'abnegate-agent')

    def test_the_innermost_crate_owns_a_nested_file(self) -> None:
        nested = metadata({'outer': [], 'inner': []})
        for package in nested['packages']:
            if package['name'] == 'inner':
                package['manifest_path'] = f'{ROOT}/crates/outer/inner/Cargo.toml'
        found = affected_crates.Workspace.from_metadata(nested)
        self.assertEqual(found.owner('crates/outer/inner/src/lib.rs').name, 'inner')
        self.assertEqual(found.owner('crates/outer/src/lib.rs').name, 'outer')

    def test_a_name_cargo_would_refuse_is_never_selected(self) -> None:
        for name in ('abnegate-exec; true', 'abnegate exec', '--workspace', '-p', ''):
            with self.subTest(name=name), self.assertRaisesRegex(affected_crates.SelectionError, 'not a cargo package name'):
                affected_crates.Workspace.from_metadata(metadata({name: []}))

    def test_a_registry_dependency_is_not_a_workspace_edge(self) -> None:
        self.assertEqual(workspace().crates['abnegate-secret'].dependencies, frozenset())

    def assertSame(self, actual: object, expected: object) -> None:
        self.assertEqual(type(actual), type(expected))
        self.assertEqual(actual, expected)


class FilesTest(unittest.TestCase):
    def select(self, *files: str) -> list[str]:
        return sorted(affected_crates.select_by_files(workspace(), files).packages)

    def test_a_leaf_crate_selects_only_itself(self) -> None:
        self.assertEqual(self.select('crates/abnegate-search/README.md'), ['abnegate-search'])

    def test_a_crate_selects_every_transitive_dependent(self) -> None:
        self.assertEqual(
            self.select('crates/abnegate-secret/src/lib.rs'),
            ['abnegate-agent', 'abnegate-agent-cli', 'abnegate-exec', 'abnegate-llm', 'abnegate-search', 'abnegate-secret'],
        )

    def test_an_optional_dependency_selects_its_dependent(self) -> None:
        self.assertEqual(self.select('crates/abnegate-agent-cli/src/lib.rs'), ['abnegate-agent', 'abnegate-agent-cli'])

    def test_a_dev_dependency_selects_its_dependent(self) -> None:
        self.assertEqual(self.select('crates/abnegate-http/src/lib.rs'), ['abnegate-agent', 'abnegate-agent-cli', 'abnegate-http', 'abnegate-llm'])

    def test_a_build_dependency_selects_its_dependent(self) -> None:
        self.assertEqual(self.select('crates/abnegate-vision/src/lib.rs'), ['abnegate-comfy', 'abnegate-vision'])

    def test_separate_crates_select_their_union(self) -> None:
        self.assertEqual(
            self.select('crates/abnegate-search/src/lib.rs', 'crates/abnegate-vision/Cargo.toml'),
            ['abnegate-comfy', 'abnegate-search', 'abnegate-vision'],
        )

    def test_every_workspace_wide_file_selects_every_crate(self) -> None:
        for file in (
            'Cargo.toml',
            'Cargo.lock',
            'rust-toolchain.toml',
            'deny.toml',
            'rustfmt.toml',
            '.cargo/config.toml',
            '.github/workflows/ci.yml',
            'scripts/affected_crates.py',
        ):
            with self.subTest(file=file):
                self.assertEqual(self.select('README.md', file), EVERY)

    def test_a_file_under_crates_in_no_crate_selects_every_crate(self) -> None:
        self.assertEqual(self.select('crates/abnegate-removed/src/lib.rs'), EVERY)

    def test_a_file_outside_every_crate_selects_nothing(self) -> None:
        for file in ('README.md', 'CONTRIBUTING.md', 'LICENSE', 'release-plz.toml', '.github/CODEOWNERS', 'docs/Cargo.toml'):
            with self.subTest(file=file):
                self.assertEqual(self.select(file), [])

    def test_no_file_selects_nothing(self) -> None:
        self.assertEqual(self.select(), [])


class TagTest(unittest.TestCase):
    def select(self, tag: str) -> list[str]:
        return sorted(affected_crates.select_by_tag(workspace(), tag).packages)

    def test_a_tag_selects_its_crate_and_dependents(self) -> None:
        self.assertEqual(self.select('abnegate-exec/v0.1.0'), ['abnegate-agent', 'abnegate-agent-cli', 'abnegate-exec'])

    def test_a_hyphenated_crate_is_named_up_to_its_version(self) -> None:
        self.assertEqual(self.select('abnegate-agent-cli/v1.20.3'), ['abnegate-agent', 'abnegate-agent-cli'])

    def test_a_crate_whose_name_holds_a_v_is_named_in_full(self) -> None:
        self.assertEqual(self.select('abnegate-vision/v0.2.0-rc.1'), ['abnegate-comfy', 'abnegate-vision'])

    def test_a_tag_for_no_workspace_crate_is_refused(self) -> None:
        with self.assertRaisesRegex(affected_crates.SelectionError, 'not a workspace package'):
            self.select('serde/v1.0.0')

    def test_a_tag_without_a_version_is_refused(self) -> None:
        for tag in ('abnegate-exec', 'abnegate-exec/v', 'abnegate-exec/vnext', 'v0.1.0', 'abnegate-exec-v0.1.0'):
            with self.subTest(tag=tag), self.assertRaisesRegex(affected_crates.SelectionError, r'not <package>/v<version>'):
                self.select(tag)


class OutputsTest(unittest.TestCase):
    def outputs(self, *packages: str) -> dict[str, str]:
        selection = affected_crates.Selection(frozenset(packages), 'test')
        return affected_crates.outputs(workspace(), selection)

    def test_nothing_selected_is_a_no_op(self) -> None:
        self.assertEqual(
            self.outputs(),
            {'packages': '[]', 'flags': '', 'scope': '', 'package': '', 'any': 'false', 'all': 'false', 'exec': 'false'},
        )

    def test_some_crates_are_named(self) -> None:
        outputs = self.outputs('abnegate-search', 'abnegate-exec')
        self.assertEqual(json.loads(outputs['packages']), ['abnegate-exec', 'abnegate-search'])
        self.assertEqual(outputs['flags'], '-p abnegate-exec -p abnegate-search')
        self.assertEqual(outputs['scope'], outputs['flags'])
        self.assertEqual((outputs['any'], outputs['all'], outputs['exec']), ('true', 'false', 'true'))

    def test_the_package_scope_adds_workspace_dependencies(self) -> None:
        self.assertEqual(self.outputs('abnegate-search')['package'], '-p abnegate-search -p abnegate-secret')
        self.assertEqual(
            self.outputs('abnegate-agent-cli')['package'],
            '-p abnegate-agent-cli -p abnegate-exec -p abnegate-http -p abnegate-llm -p abnegate-secret',
        )

    def test_every_crate_is_the_workspace(self) -> None:
        outputs = self.outputs(*EVERY)
        self.assertEqual((outputs['scope'], outputs['package']), ('--workspace', '--workspace'))
        self.assertEqual((outputs['any'], outputs['all'], outputs['exec']), ('true', 'true', 'true'))
        self.assertEqual(outputs['flags'], ' '.join(f'-p {name}' for name in EVERY))


class CommandTest(unittest.TestCase):
    """Runs the script in a real git repository holding a real cargo workspace."""

    def setUp(self) -> None:
        for tool in ('cargo', 'git'):
            if shutil.which(tool) is None:
                self.fail(f'{tool} is needed to test {SCRIPT.name} end to end')
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.write('Cargo.toml', '[workspace]\nmembers = ["crates/*"]\nresolver = "3"\n')
        self.crate('base', '')
        self.crate('leaf', 'base = { path = "../base" }\n')
        self.crate('other', '')
        self.write('README.md', 'workspace\n')
        self.script = self.root / 'scripts' / SCRIPT.name
        self.script.parent.mkdir()
        shutil.copyfile(SCRIPT, self.script)
        self.git('init', '--quiet', '--initial-branch=main')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()

    def crate(self, name: str, dependencies: str) -> None:
        manifest = f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n{dependencies}'
        self.write(f'crates/{name}/Cargo.toml', manifest)
        self.write(f'crates/{name}/src/lib.rs', '')

    def write(self, file: str, content: str) -> None:
        path = self.root / file
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding='utf-8')

    def git(self, *arguments: str) -> str:
        command = ['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com', '-c', 'commit.gpgsign=false', *arguments]
        return subprocess.run(command, cwd=self.root, capture_output=True, text=True, check=True).stdout

    def commit(self) -> None:
        self.git('add', '--all')
        self.git('commit', '--quiet', '--message', 'change')

    def run_script(self, *arguments: str) -> tuple[int, dict[str, str], str]:
        result = subprocess.run([sys.executable, str(self.script), *arguments], cwd=self.root, capture_output=True, text=True, check=False)
        outputs = dict(line.split('=', 1) for line in result.stdout.splitlines())
        return result.returncode, outputs, result.stderr

    def selected(self, *arguments: str) -> list[str]:
        status, outputs, errors = self.run_script(*arguments)
        self.assertEqual(status, SELECTED, errors)
        return json.loads(outputs['packages'])

    def test_a_changed_dependency_selects_its_dependent(self) -> None:
        self.write('crates/base/src/lib.rs', 'pub fn base() {}\n')
        self.commit()
        self.assertEqual(self.selected('--base', self.base), ['base', 'leaf'])

    def test_a_changed_leaf_selects_only_itself(self) -> None:
        self.write('crates/leaf/src/lib.rs', 'pub fn leaf() {}\n')
        self.commit()
        self.assertEqual(self.selected('--base', self.base, '--head', 'HEAD'), ['leaf'])

    def test_a_file_moved_between_crates_selects_both(self) -> None:
        self.write('crates/other/src/moved.rs', '')
        self.commit()
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.git('mv', 'crates/other/src/moved.rs', 'crates/leaf/src/moved.rs')
        self.commit()
        self.assertEqual(self.selected('--base', self.base), ['leaf', 'other'])

    def test_a_changed_readme_selects_nothing(self) -> None:
        self.write('README.md', 'changed\n')
        self.commit()
        status, outputs, errors = self.run_script('--base', self.base)
        self.assertEqual(status, SELECTED, errors)
        self.assertEqual((outputs['packages'], outputs['any'], outputs['scope']), ('[]', 'false', ''))
        self.assertIn('no file changed belongs to a crate', errors)

    def test_a_changed_workspace_manifest_selects_the_workspace(self) -> None:
        self.write('Cargo.toml', '[workspace]\nmembers = ["crates/*"]\nresolver = "3"\n\n[workspace.package]\n')
        self.commit()
        status, outputs, errors = self.run_script('--base', self.base)
        self.assertEqual(status, SELECTED, errors)
        self.assertEqual((outputs['scope'], outputs['all']), ('--workspace', 'true'))

    def test_all_selects_every_crate(self) -> None:
        self.assertEqual(self.selected('--all'), ['base', 'leaf', 'other'])

    def test_a_tag_selects_its_crate_and_dependents(self) -> None:
        self.assertEqual(self.selected('--tag', 'base/v0.1.0'), ['base', 'leaf'])

    def test_an_unknown_commit_is_an_error(self) -> None:
        status, _, errors = self.run_script('--base', 'does-not-exist')
        self.assertEqual(status, ERROR)
        self.assertIn('git diff', errors)

    def test_a_source_is_required(self) -> None:
        status, _, errors = self.run_script()
        self.assertEqual(status, ERROR)
        self.assertIn('one of the arguments', errors)

    def test_an_old_python_is_refused(self) -> None:
        program = textwrap.dedent(f"""
            import runpy
            import sys

            sys.version_info = (3, 8, 0)
            sys.argv = ['{SCRIPT.name}', '--all']
            runpy.run_path({str(self.script)!r}, run_name='__main__')
        """)
        result = subprocess.run([sys.executable, '-c', program], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, ERROR)
        self.assertIn('needs Python 3.9 or later', result.stderr)


if __name__ == '__main__':
    unittest.main()
