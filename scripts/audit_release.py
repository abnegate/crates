#!/usr/bin/env python3
r"""Check that release-plz.toml cannot publish a crate ahead of its changelog.

release-plz publishes and tags a crate on whichever commit its release job
runs, and sizes the next release from the commits made since. Two settings in
the [workspace] table of release-plz.toml keep those two in step, and the
commit parsers in its [changelog] table keep release commits out of changelogs.

`release_always = false` has the release job act only on a commit merged from
the release pull request, so a crate is published and tagged on the commit that
carries its changelog. Otherwise a version bumped by hand is published at once,
its changelog arrives in a later commit, and every crate that commit touches
looks changed since its release.

`release_commits` is the pattern a commit message must match before release-plz
prepares a release for the crates the commit touches. It must hold back a
release commit, any other `chore` and a merge commit, and let through every
other type, a breaking change of any type and the commit release-plz itself
invents when a crate's dependencies change. A commit it holds back also holds
back that invented commit, so the fewer it holds, the fewer dependency changes
wait unpublished.

`commit_parsers` replaces release-plz's defaults once it names any parser. Its
first entry must skip a release commit, both as written and once release-plz
has turned `(#79)` into a link, and no other commit. Its last must be the `.*`
catch-all, which groups every commit no earlier entry claims.

release-plz compiles each pattern with Rust's regex crate and this audit with
Python's re, so a pattern must stay within the syntax both read alike:
literals, `.`, `^`, sets, groups, `|` and the `?`, `*`, `+` and `{n,m}`
quantifiers. Anything else, such as lookaround, a backreference, an inline
flag or a class like `\d`, is refused rather than judged. The skip pattern may
also use `\n` and `$`, which it needs to end a subject: a changelog parser
reads a trimmed message, and in a text that does not end in a newline `$` is
its end to both engines. `release_commits` may use neither.

Each problem is printed as `release-plz.toml: problem`. The exit status is 0
when there is none, 1 when there is one, and 2 when the audit could not finish,
which proves nothing either way: release-plz.toml is missing or is not TOML, the
script itself failed, or Python is older than 3.11.
"""

from __future__ import annotations

import re
import sys
import traceback
from enum import IntEnum
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
CONFIGURATION = ROOT / 'release-plz.toml'
SCRIPT = Path(__file__).name
MINIMUM_PYTHON = (3, 11)

WORKSPACE = 'workspace'
ALWAYS = 'release_always'
COMMITS = 'release_commits'
CHANGELOG = 'changelog'
PARSERS = 'commit_parsers'
MESSAGE = 'message'
SKIP = 'skip'
CATCHALL = '.*'

RELEASED = (
    'feat: add a rerank provider',
    "feat(agent): connect streamable-http mcp servers and report each server's outcome (#81)",
    'fix(vcs): refuse a configured_origin path that is not a regular file (#45)',
    'perf(recurrence): jump to the current period instead of walking to it (#69)',
    'refactor(config): let error variants gain a field in a minor release',
    'docs(comfy): so the changelog marks every change to released behaviour as breaking',
    'feat!: drop the constructor nothing calls',
    'fix(deps)!: move rmcp past its advisories and drop the rusqlite feature (#78)',
    'chore(deps)!: raise the minimum supported rust version',
    'chore: update Cargo.toml dependencies',
    'chore: update Cargo.lock dependencies',
    'test(secret): read the plaintext report in a process no other test shares (#47)',
    'test: cover the retry budget',
    'style(agent): space the rmcp dev-dependency like its siblings',
    'style: one import per statement',
    'ci: install ripgrep so the agent test runs on linux',
    'build: pin the toolchain the release job installs',
    'revert: let error variants gain a field in a minor release',
)
RELEASES = (
    'chore: release',
    'chore: release (#79)',
    'chore: release v0.1.13 (#62)',
    'chore(abnegate-vcs): release v0.1.13 (#77)',
    'chore: release (#79)\n\nfeat(agent): connect streamable-http mcp servers (#81)',
)
HELD = (
    *RELEASES,
    'chore(deps): prune the workspace dependencies no crate takes',
    'Merge pull request #11 from abnegate/release-plz-2026-10-01T09-54-53Z',
)
LISTED = tuple(message for message in (*RELEASED, *HELD) if message not in RELEASES)

REFERENCE = re.compile(r'\(#([0-9]+)\)')
LINK = r'([#\1](https://github.com/abnegate/crates/pull/\1))'

PUNCTUATION = r'\\^$.|?*+()\[\]{}-'
NEWLINE = 'n'
START = r'\^'
END = r'\$'


def grammar(escaped: str, anchors: str) -> re.Pattern[str]:
    escape = rf'\\[{escaped}]'
    return re.compile(
        rf'''
        (?P<quantifier>(?:[?*+]|\{{\d+(?:,\d*)?\}})\??)
        | (?P<set>\[\^?(?:[^\\\[\]]|{escape})+\])
        | (?P<opener>\((?:\?:)?(?!\?)|\||{anchors})
        | (?P<atom>{escape}|[).]|[^\\^$.|?*+()\[\]{{}}])
        ''',
        re.VERBOSE,
    )


SHARED = grammar(PUNCTUATION, START)
TRIMMED = grammar(NEWLINE + PUNCTUATION, f'{START}|{END}')
QUANTIFIER = 'quantifier'
SET = 'set'
OPENER = 'opener'
SET_OPERATORS = ('&&', '--', '~~')

EAGER = (
    f'`{ALWAYS}` must be false in [{WORKSPACE}]: release-plz otherwise publishes and tags a crate on every push, '
    'ahead of the release pull request that writes its changelog'
)
UNFILTERED = (
    f'`{COMMITS}` must be a pattern in [{WORKSPACE}]: without one every commit prepares a release, a release '
    'commit included'
)
UNPARSED = (
    f'`{PARSERS}` must be a list of tables in [{CHANGELOG}]: without one release-plz lists every release commit in '
    'the changelogs it writes'
)
UNSKIPPED = (
    f'the first entry of `{PARSERS}` must have `{SKIP} = true` and a `{MESSAGE}` pattern: it keeps release commits '
    'out of changelogs, and an entry ahead of it claims them first'
)
UNGROUPED = (
    f"the last entry of `{PARSERS}` must be the `{CATCHALL}` catch-all and must not skip: it groups every commit no "
    'earlier entry claims'
)
SKIPPER = f'the first `{PARSERS}` pattern'


class Exit(IntEnum):
    PASSED = 0
    PROBLEMS = 1
    ERROR = 2


class AuditError(Exception):
    pass


def load(path: Path) -> dict[str, Any]:
    # Imported here so that main() can refuse a Python too old to have tomllib.
    import tomllib

    try:
        with path.open('rb') as file:
            return tomllib.load(file)
    except OSError as error:
        raise AuditError(f'cannot read {path.name}: {error}') from error
    except tomllib.TOMLDecodeError as error:
        raise AuditError(f'{path.name} is not TOML: {error}') from error


def unshared(pattern: str, syntax: re.Pattern[str] = SHARED) -> int | None:
    repeatable = False
    index = 0
    while index < len(pattern):
        token = syntax.match(pattern, index)
        if token is None:
            return index
        if token.lastgroup == QUANTIFIER:
            if not repeatable:
                return index
            repeatable = False
        elif token.lastgroup == SET and any(operator in token[0] for operator in SET_OPERATORS):
            return index
        else:
            repeatable = token.lastgroup != OPENER
        index = token.end()
    return None


def subject(message: str) -> str:
    first, _, body = message.partition('\n')
    return f'`{first}` followed by a body' if body else f'`{first}`'


def unreadable(name: str, pattern: str, syntax: re.Pattern[str]) -> str | None:
    index = unshared(pattern, syntax)
    if index is not None:
        return (
            f"{name} leaves the syntax Rust's regex crate and Python's re read alike at "
            f'`{pattern[index:]}` (character {index + 1}), so it cannot be judged'
        )
    try:
        re.compile(pattern)
    except re.error as error:
        return f'{name} is not a pattern: {error}'
    return None


def misjudged(pattern: str) -> list[str]:
    problem = unreadable(f'`{COMMITS}`', pattern, SHARED)
    if problem is not None:
        return [problem]
    compiled = re.compile(pattern)
    return [
        *(
            f'`{COMMITS}` lets {subject(message)} prepare a release, which it must hold back'
            for message in HELD
            if compiled.search(message) is not None
        ),
        *(
            f'`{COMMITS}` holds back {subject(message)}, which must prepare a release'
            for message in RELEASED
            if compiled.search(message) is None
        ),
    ]


def forms(message: str) -> tuple[str, ...]:
    linked = REFERENCE.sub(LINK, message)
    return (message,) if linked == message else (message, linked)


def misskipped(pattern: str) -> list[str]:
    problem = unreadable(SKIPPER, pattern, TRIMMED)
    if problem is not None:
        return [problem]
    compiled = re.compile(pattern)
    return [
        *(
            f'{SKIPPER} does not skip {subject(form)}, a release commit'
            for message in RELEASES
            for form in forms(message)
            if compiled.search(form) is None
        ),
        *(
            f'{SKIPPER} skips {subject(form)}, which is not a release commit'
            for message in LISTED
            for form in forms(message)
            if compiled.search(form) is not None
        ),
    ]


def mislisted(parsers: object) -> list[str]:
    if not isinstance(parsers, list) or not parsers or not all(isinstance(parser, dict) for parser in parsers):
        return [UNPARSED]
    problems: list[str] = []
    pattern = parsers[0].get(MESSAGE)
    if parsers[0].get(SKIP) is True and isinstance(pattern, str):
        problems.extend(misskipped(pattern))
    else:
        problems.append(UNSKIPPED)
    if parsers[-1].get(MESSAGE) != CATCHALL or parsers[-1].get(SKIP) is True:
        problems.append(UNGROUPED)
    return problems


def table(configuration: dict[str, Any], name: str) -> dict[str, Any]:
    value = configuration.get(name)
    return value if isinstance(value, dict) else {}


def audit(configuration: dict[str, Any]) -> list[str]:
    workspace = table(configuration, WORKSPACE)
    problems: list[str] = []
    if workspace.get(ALWAYS) is not False:
        problems.append(EAGER)
    pattern = workspace.get(COMMITS)
    if isinstance(pattern, str):
        problems.extend(misjudged(pattern))
    else:
        problems.append(UNFILTERED)
    problems.extend(mislisted(table(configuration, CHANGELOG).get(PARSERS)))
    return problems


def run() -> Exit:
    problems = audit(load(CONFIGURATION))
    for problem in problems:
        print(f'{CONFIGURATION.name}: {problem}')
    sys.stdout.flush()
    if problems:
        print(
            f'{len(problems)} problem(s) above with what {CONFIGURATION.name} releases and when '
            '(CONTRIBUTING.md, Releases).',
            file=sys.stderr,
        )
    return Exit.PROBLEMS if problems else Exit.PASSED


def version(parts: tuple[object, ...]) -> str:
    return '.'.join(str(part) for part in parts[:3])


def main() -> Exit:
    if sys.version_info < MINIMUM_PYTHON:
        print(
            f'{SCRIPT}: needs Python {version(MINIMUM_PYTHON)} or later, but this is Python '
            f'{version(sys.version_info)}; nothing was audited.',
            file=sys.stderr,
        )
        return Exit.ERROR
    try:
        return run()
    except AuditError as error:
        print(f'{SCRIPT}: {error}', file=sys.stderr)
    except Exception:
        traceback.print_exc()
        print(f'{SCRIPT}: the audit failed on the error above and proves nothing.', file=sys.stderr)
    return Exit.ERROR


if __name__ == '__main__':
    sys.exit(main())
