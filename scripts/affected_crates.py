#!/usr/bin/env python3
"""Select the workspace crates a change can affect, so CI checks only those.

A file inside a crate's directory affects that crate. A workspace-wide file, or
a file under crates/ that no crate owns, affects every crate. Any other file,
such as the root README, affects none. Every crate that depends on an affected
crate, directly or transitively, through a normal, dev or build dependency,
optional or not, is affected too.

A release tag, `<package>-v<version>`, affects that package and its dependents.

Each result is printed as a GitHub Actions output, `name=value`:

  packages  the affected packages, as a JSON list
  flags     `-p <package>` for each affected package
  scope     `--workspace` when every crate is affected, else `flags`
  package   the scope for `cargo package`, which also names every workspace
            crate the affected ones depend on: cargo verifies a package against
            the registry unless the crate it needs is packaged alongside it
  any       whether any crate is affected
  all       whether every crate is affected
  exec      whether abnegate-exec is affected

The exit status is 0 when the selection was made and 2 when it could not be:
git or cargo failed, the tag names no workspace package, a package name is not
one cargo accepts and so is unsafe to splice into a command, or Python is older
than 3.9.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import traceback
from collections.abc import Callable
from collections.abc import Iterable
from dataclasses import dataclass
from enum import IntEnum
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = Path(__file__).name
MINIMUM_PYTHON = (3, 9)

CRATES = 'crates/'
PACKAGE = re.compile(r'[A-Za-z_][A-Za-z0-9_-]*')
SANDBOXED = 'abnegate-exec'
TAG = re.compile(r'(?P<package>.+)-v(?P<version>\d+\.\d+\.\d+\S*)')
WORKSPACE_FILES = frozenset({
    'Cargo.lock',
    'Cargo.toml',
    'deny.toml',
    'rust-toolchain.toml',
    'rustfmt.toml',
})
WORKSPACE_DIRECTORIES = ('.cargo/', '.github/workflows/', 'scripts/')


class Exit(IntEnum):
    SELECTED = 0
    ERROR = 2


class SelectionError(Exception):
    pass


@dataclass(frozen=True)
class Crate:
    name: str
    directory: str
    dependencies: frozenset[str]


@dataclass(frozen=True)
class Selection:
    packages: frozenset[str]
    reason: str


class Workspace:
    def __init__(self, crates: Iterable[Crate]) -> None:
        self.crates = {crate.name: crate for crate in crates}
        for name in self.crates:
            if PACKAGE.fullmatch(name) is None:
                raise SelectionError(f'{name!r} is not a cargo package name, so it cannot be passed to cargo as -p')
        self.names = frozenset(self.crates)
        self.dependents: dict[str, set[str]] = {name: set() for name in self.names}
        for crate in self.crates.values():
            for dependency in crate.dependencies:
                self.dependents[dependency].add(crate.name)

    @classmethod
    def from_metadata(cls, metadata: dict[str, Any]) -> Workspace:
        root = Path(metadata['workspace_root'])
        members = set(metadata['workspace_members'])
        packages = [package for package in metadata['packages'] if package['id'] in members]
        directories = {Path(package['manifest_path']).parent: package['name'] for package in packages}
        return cls(
            Crate(
                name=package['name'],
                directory=Path(package['manifest_path']).parent.relative_to(root).as_posix(),
                dependencies=frozenset(
                    directories[Path(dependency['path'])]
                    for dependency in package['dependencies']
                    if dependency.get('path') and Path(dependency['path']) in directories
                ),
            )
            for package in packages
        )

    def owner(self, file: str) -> Crate | None:
        owners = [crate for crate in self.crates.values() if file.startswith(f'{crate.directory}/')]
        return max(owners, key=lambda crate: len(crate.directory), default=None)

    def with_dependents(self, names: Iterable[str]) -> frozenset[str]:
        return self.closure(names, lambda name: self.dependents[name])

    def with_dependencies(self, names: Iterable[str]) -> frozenset[str]:
        return self.closure(names, lambda name: self.crates[name].dependencies)

    @staticmethod
    def closure(names: Iterable[str], neighbours: Callable[[str], Iterable[str]]) -> frozenset[str]:
        reached = set(names)
        pending = list(reached)
        while pending:
            for neighbour in neighbours(pending.pop()):
                if neighbour not in reached:
                    reached.add(neighbour)
                    pending.append(neighbour)
        return frozenset(reached)

    def scope(self, names: frozenset[str]) -> str:
        return '--workspace' if names == self.names else flags(names)


def flags(names: Iterable[str]) -> str:
    return ' '.join(f'-p {name}' for name in sorted(names))


def is_workspace_wide(file: str) -> bool:
    return file in WORKSPACE_FILES or file.startswith(WORKSPACE_DIRECTORIES)


def select_everything(workspace: Workspace, reason: str) -> Selection:
    return Selection(workspace.names, reason)


def select_by_files(workspace: Workspace, files: Iterable[str]) -> Selection:
    changed: set[str] = set()
    for file in files:
        if is_workspace_wide(file):
            return select_everything(workspace, f'{file} is workspace-wide')
        crate = workspace.owner(file)
        if crate is not None:
            changed.add(crate.name)
        elif file.startswith(CRATES):
            return select_everything(workspace, f'{file} is under {CRATES} but in no crate')
    if not changed:
        return Selection(frozenset(), 'no file changed belongs to a crate')
    return Selection(workspace.with_dependents(changed), f'changed: {", ".join(sorted(changed))}')


def select_by_tag(workspace: Workspace, tag: str) -> Selection:
    match = TAG.fullmatch(tag)
    if match is None:
        raise SelectionError(f'the tag {tag} is not <package>-v<version>')
    package = match['package']
    if package not in workspace.names:
        raise SelectionError(f'the tag {tag} names {package}, which is not a workspace package')
    return Selection(workspace.with_dependents({package}), f'tagged: {package}')


def outputs(workspace: Workspace, selection: Selection) -> dict[str, str]:
    packages = selection.packages
    return {
        'packages': json.dumps(sorted(packages)),
        'flags': flags(packages),
        'scope': workspace.scope(packages),
        'package': workspace.scope(workspace.with_dependencies(packages)),
        'any': json.dumps(bool(packages)),
        'all': json.dumps(packages == workspace.names),
        'exec': json.dumps(SANDBOXED in packages),
    }


def execute(command: list[str]) -> str:
    try:
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)
    except OSError as error:
        raise SelectionError(f'cannot run {command[0]}: {error}') from error
    if result.returncode != 0:
        raise SelectionError(f'`{" ".join(command)}` failed:\n{result.stderr.strip()}')
    return result.stdout


def load_workspace() -> Workspace:
    metadata = execute(['cargo', 'metadata', '--no-deps', '--format-version', '1', '--manifest-path', str(ROOT / 'Cargo.toml')])
    return Workspace.from_metadata(json.loads(metadata))


def changed_files(base: str, head: str) -> list[str]:
    difference = execute(['git', 'diff', '--name-only', '--no-renames', '-z', f'{base}...{head}'])
    return [file for file in difference.split('\0') if file]


def run() -> Exit:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--all', action='store_true', help='select every crate')
    source.add_argument('--tag', help='select the crate a release tag names, such as abnegate-exec-v0.1.0')
    source.add_argument('--base', help='select the crates changed since the merge base of this commit and --head')
    parser.add_argument('--head', default='HEAD', help='the commit compared with --base (default: HEAD)')
    arguments = parser.parse_args()
    workspace = load_workspace()
    if arguments.all:
        selection = select_everything(workspace, 'every crate was asked for')
    elif arguments.tag is not None:
        selection = select_by_tag(workspace, arguments.tag)
    else:
        selection = select_by_files(workspace, changed_files(arguments.base, arguments.head))
    print(f'{selection.reason}; checking {len(selection.packages)} of {len(workspace.names)} crates', file=sys.stderr)
    for name, value in outputs(workspace, selection).items():
        print(f'{name}={value}')
    return Exit.SELECTED


def version(parts: tuple[object, ...]) -> str:
    return '.'.join(str(part) for part in parts[:3])


def main() -> Exit:
    if sys.version_info < MINIMUM_PYTHON:
        print(
            f'{SCRIPT}: needs Python {version(MINIMUM_PYTHON)} or later, but this is Python '
            f'{version(sys.version_info)}; nothing was selected.',
            file=sys.stderr,
        )
        return Exit.ERROR
    try:
        return run()
    except SelectionError as error:
        print(f'{SCRIPT}: {error}', file=sys.stderr)
    except Exception:
        traceback.print_exc()
        print(f'{SCRIPT}: the selection failed on the error above.', file=sys.stderr)
    return Exit.ERROR


if __name__ == '__main__':
    sys.exit(main())
