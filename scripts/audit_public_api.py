#!/usr/bin/env python3
"""Find public types that cannot gain a field without a breaking release.

A caller can build a struct whose fields are public, or a struct variant of a
public enum, with a literal, and can match it without `..`, so adding a field
to it breaks that caller. Each must be #[non_exhaustive] unless ALLOWED names
it. Only what a caller can reach is audited: code compiled only for tests,
items that are not `pub`, and `pub` items of a private module that nothing
re-exports are skipped.

With no crate named, every crate under crates/ is audited. Each offender is
printed as `path:line: item`, and the exit status is 1 when there is one.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / 'crates'

ALLOWED = frozenset({
    ('crates/abnegate-vision/src/gravity/point.rs', 'Point'),
    ('crates/abnegate-vision/src/gravity/rectangle.rs', 'Rectangle'),
})

SYSROOT_CRATES = frozenset({'alloc', 'core', 'proc_macro', 'std', 'test'})
OPENERS = frozenset({'(', '[', '{'})
CLOSERS = frozenset({')', ']', '}'})
RESTRICTIONS = frozenset({'crate', 'self', 'super'})
QUALIFIERS = frozenset({'async', 'auto', 'default', 'safe', 'unsafe'})
FUNCTION_QUALIFIERS = frozenset({'async', 'extern', 'fn', 'unsafe'})
NAMED_ITEMS = frozenset({'const', 'fn', 'static', 'trait', 'type', 'union'})
VALUE_ITEMS = frozenset({'const', 'static', 'type', 'use'})
BODIES = frozenset({'(', '{', ';'})
BRACED_BODIES = frozenset({'{', ';'})
SEMICOLON = frozenset({';'})

TOKEN = re.compile(
    r'''
    (?P<space>\s+)
    | (?P<comment>//[^\n]*)
    | (?P<block>/\*)
    | (?P<literal>
        (?:br|cr|r)(?P<hashes>\#*)".*?"(?P=hashes)
        | [bc]?"(?:\\.|[^"\\])*"
        | b?'(?:[^\\'\n]|\\(?:u\{[0-9a-fA-F_]*\}|x[0-9a-fA-F]{2}|.))'
      )
    | (?P<word>'?(?:r\#)?[^\W\d]\w*|\d\w*)
    | (?P<punctuation>->|=>|::|.)
    ''',
    re.VERBOSE | re.DOTALL,
)
BLOCK_DELIMITER = re.compile(r'/\*|\*/')


class AuditError(Exception):
    pass


class SourceError(Exception):
    def __init__(self, line: int, problem: str) -> None:
        super().__init__(f'{line}: {problem}')


@dataclass(frozen=True)
class Token:
    text: str
    line: int


@dataclass(frozen=True, order=True)
class Offender:
    path: str
    line: int
    item: str

    def __str__(self) -> str:
        return f'{self.path}:{self.line}: {self.item}'


@dataclass(eq=False)
class Item:
    offenders: tuple[Offender, ...] = ()


@dataclass(eq=False)
class External:
    pass


@dataclass(eq=False)
class Import:
    module: Module
    segments: tuple[str, ...]
    location: str


@dataclass(eq=False)
class Alias:
    module: Module
    segments: tuple[str, ...]


@dataclass(frozen=True)
class Binding:
    public: bool
    target: Module | Item | External | Import | Alias


@dataclass(eq=False)
class Module:
    file: Path
    directory: Path
    inline: bool
    parent: Module | None
    external: frozenset[str]
    bindings: dict[str, list[Binding]] = field(default_factory=dict)
    globs: list[Binding] = field(default_factory=list)

    def bind(self, name: str, public: bool, target: Module | Item | External | Import | Alias) -> None:
        self.bindings.setdefault(name, []).append(Binding(public, target))

    def root(self) -> Module:
        return self if self.parent is None else self.parent.root()

    def location(self, token: Token) -> str:
        return f'{relative(self.file)}:{token.line}'


Target = Module | Item | External
TYPE_ALIAS = Item()


def relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def tokenize(source: str) -> list[Token]:
    tokens: list[Token] = []
    position = 0
    line = 1
    while position < len(source):
        match = TOKEN.match(source, position)
        if match.group('block') is not None:
            end = block_comment_end(source, position, line)
        else:
            end = match.end()
            if match.group('word') is not None:
                tokens.append(Token(match.group().removeprefix('r#'), line))
            elif match.group('space') is None and match.group('comment') is None:
                tokens.append(Token(match.group(), line))
        line += source.count('\n', position, end)
        position = end
    return tokens


def block_comment_end(source: str, position: int, line: int) -> int:
    depth = 0
    for match in BLOCK_DELIMITER.finditer(source, position):
        depth += 1 if match.group() == '/*' else -1
        if depth == 0:
            return match.end()
    raise SourceError(line, 'unterminated block comment')


def closing(tokens: list[Token], position: int) -> int:
    depth = 0
    for index in range(position, len(tokens)):
        text = tokens[index].text
        if text in OPENERS:
            depth += 1
        elif text in CLOSERS:
            depth -= 1
            if depth == 0:
                return index
    raise SourceError(tokens[position].line, f'{tokens[position].text} is never closed')


def find(tokens: list[Token], position: int, targets: frozenset[str]) -> int:
    start = position
    while position < len(tokens):
        text = tokens[position].text
        if text in targets:
            return position
        position = closing(tokens, position) + 1 if text in OPENERS else position + 1
    raise SourceError(tokens[start].line, f'expected one of {sorted(targets)}')


def separator(tokens: list[Token], position: int, end: int, angled: bool) -> int:
    angles = 0
    while position < end:
        text = tokens[position].text
        if text in OPENERS:
            position = closing(tokens, position) + 1
            continue
        if angled and text == '<':
            angles += 1
        elif angled and text == '>':
            angles -= 1
        elif text == ',' and angles == 0:
            return position
        position += 1
    return end


def after_generics(tokens: list[Token], position: int) -> int:
    if tokens[position].text != '<':
        return position
    depth = 0
    while position < len(tokens):
        text = tokens[position].text
        if text in OPENERS:
            position = closing(tokens, position) + 1
            continue
        if text == '<':
            depth += 1
        elif text == '>':
            depth -= 1
            if depth == 0:
                return position + 1
        position += 1
    raise SourceError(tokens[position - 1].line, 'generics are never closed')


def read_attributes(tokens: list[Token], position: int, end: int) -> tuple[list[list[Token]], int]:
    attributes: list[list[Token]] = []
    while position + 1 < end and tokens[position].text == '#' and tokens[position + 1].text == '[':
        close = closing(tokens, position + 1)
        attributes.append(tokens[position + 2:close])
        position = close + 1
    return attributes, position


def read_visibility(tokens: list[Token], position: int) -> tuple[bool, int]:
    if tokens[position].text != 'pub':
        return False, position
    following = position + 1
    if following + 2 < len(tokens) and tokens[following].text == '(':
        restriction = tokens[following + 1].text
        if restriction == 'in' or (restriction in RESTRICTIONS and tokens[following + 2].text == ')'):
            return False, closing(tokens, following) + 1
    return True, following


def non_exhaustive(attributes: list[list[Token]]) -> bool:
    return any(len(attribute) == 1 and attribute[0].text == 'non_exhaustive' for attribute in attributes)


def test_only(attributes: list[list[Token]]) -> bool:
    return any(
        len(attribute) > 2
        and attribute[0].text == 'cfg'
        and attribute[1].text == '('
        and predicate(attribute, 2)[0] is False
        for attribute in attributes
    )


def predicate(tokens: list[Token], position: int) -> tuple[bool | None, int]:
    """Whether a cfg predicate holds outside tests, or None when that depends on the build."""
    name = tokens[position].text
    position += 1
    if position < len(tokens) and tokens[position].text == '=':
        return None, position + 2
    if position >= len(tokens) or tokens[position].text != '(':
        return {'test': False, 'false': False, 'true': True}.get(name), position
    close = closing(tokens, position)
    values: list[bool | None] = []
    position += 1
    while position < close:
        value, position = predicate(tokens, position)
        values.append(value)
        if tokens[position].text == ',':
            position += 1
    if name == 'all':
        result = False if False in values else (True if all(values) else None)
    elif name == 'any':
        result = True if True in values else (False if all(value is False for value in values) else None)
    elif name == 'not' and len(values) == 1 and values[0] is not None:
        result = not values[0]
    else:
        result = None
    return result, close + 1


def literal_text(token: Token) -> str:
    return token.text.lstrip('bcr').strip('#')[1:-1]


def parse_file(module: Module) -> None:
    try:
        tokens = tokenize(module.file.read_text())
        parse_module(module, tokens, 0, len(tokens))
    except SourceError as error:
        raise AuditError(f'{relative(module.file)}:{error}') from None


def parse_module(module: Module, tokens: list[Token], position: int, end: int) -> None:
    while position < end:
        if tokens[position].text == '#' and position + 1 < end and tokens[position + 1].text == '!':
            close = closing(tokens, position + 2)
            if test_only([tokens[position + 3:close]]):
                return
            position = close + 1
            continue
        attributes, position = read_attributes(tokens, position, end)
        if position >= end:
            return
        if tokens[position].text == ';':
            position += 1
            continue
        public, position = read_visibility(tokens, position)
        if test_only(attributes):
            position = item_end(tokens, past_qualifiers(tokens, position), end)
            continue
        position = parse_item(module, tokens, position, end, public, attributes)


def following_text(tokens: list[Token], position: int) -> str:
    return tokens[position + 1].text if position + 1 < len(tokens) else ''


def past_qualifiers(tokens: list[Token], position: int) -> int:
    while True:
        text = tokens[position].text
        following = following_text(tokens, position)
        if text in QUALIFIERS:
            position += 1
        elif text == 'extern' and following != 'crate':
            position += 2 if following.startswith('"') else 1
        elif text == 'const' and following in FUNCTION_QUALIFIERS:
            position += 1
        else:
            return position


def item_end(tokens: list[Token], position: int, end: int) -> int:
    value = tokens[position].text in VALUE_ITEMS
    while position < end:
        text = tokens[position].text
        if text == ';':
            return position + 1
        if text in OPENERS:
            close = closing(tokens, position)
            if text == '{' and not value:
                return close + 1
            position = close + 1
            continue
        position += 1
    return end


def parse_item(
    module: Module,
    tokens: list[Token],
    position: int,
    end: int,
    public: bool,
    attributes: list[list[Token]],
) -> int:
    position = past_qualifiers(tokens, position)
    keyword = tokens[position].text
    following = following_text(tokens, position)
    if keyword == 'mod':
        return parse_mod(module, tokens, position, public, attributes)
    if keyword == 'use':
        return parse_use(module, tokens, position, public)
    if keyword in ('struct', 'enum'):
        return parse_type(module, tokens, position, public, attributes)
    if keyword == 'extern' and following == 'crate':
        name = tokens[position + 4] if tokens[position + 3].text == 'as' else tokens[position + 2]
        module.bind(name.text, public, External())
    elif keyword == 'macro_rules' and following == '!':
        module.bind(tokens[position + 2].text, public, Item())
    elif keyword == 'type':
        module.bind(following, public, Alias(module, type_path(tokens, position + 1)))
    elif keyword in NAMED_ITEMS:
        module.bind(tokens[position + 2].text if following == 'mut' else following, public, Item())
    return item_end(tokens, position, end)


def type_path(tokens: list[Token], position: int) -> tuple[str, ...]:
    position = after_generics(tokens, position + 1)
    if tokens[position].text != '=':
        return ()
    segments: list[str] = []
    position += 1
    if tokens[position].text == '::':
        segments.append('::')
        position += 1
    while re.fullmatch(r'[^\W\d]\w*', tokens[position].text):
        segments.append(tokens[position].text)
        if tokens[position + 1].text != '::':
            return tuple(segments)
        position += 2
    return ()


def parse_mod(
    module: Module,
    tokens: list[Token],
    position: int,
    public: bool,
    attributes: list[list[Token]],
) -> int:
    name = tokens[position + 1]
    body = position + 2
    if tokens[body].text == '{':
        close = closing(tokens, body)
        child = Module(module.file, module.directory / name.text, True, module, module.external)
        module.bind(name.text, public, child)
        parse_module(child, tokens, body + 1, close)
        return close + 1
    file = module_file(module, name, attributes)
    directory = file.parent if file.name == 'mod.rs' or path_attribute(attributes) else file.with_suffix('')
    child = Module(file, directory, False, module, module.external)
    module.bind(name.text, public, child)
    parse_file(child)
    return body + 1


def path_attribute(attributes: list[list[Token]]) -> str | None:
    for attribute in attributes:
        if len(attribute) == 3 and attribute[0].text == 'path' and attribute[1].text == '=':
            return literal_text(attribute[2])
    return None


def module_file(module: Module, name: Token, attributes: list[list[Token]]) -> Path:
    override = path_attribute(attributes)
    if override is not None:
        candidates = [(module.directory if module.inline else module.file.parent) / override]
    else:
        candidates = [module.directory / f'{name.text}.rs', module.directory / name.text / 'mod.rs']
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise AuditError(f'{module.location(name)}: no file for `mod {name.text};`')


def parse_use(module: Module, tokens: list[Token], position: int, public: bool) -> int:
    semicolon = find(tokens, position, SEMICOLON)
    location = module.location(tokens[position])
    for segments, alias, glob in use_tree(tokens, position + 1, semicolon, ()):
        if glob:
            module.globs.append(Binding(public, Import(module, segments, location)))
            continue
        if segments[-1] == 'self':
            segments = segments[:-1]
        name = alias or segments[-1]
        if name != '_':
            module.bind(name, public, Import(module, segments, location))
    return semicolon + 1


def use_tree(
    tokens: list[Token],
    position: int,
    end: int,
    prefix: tuple[str, ...],
) -> Iterator[tuple[tuple[str, ...], str | None, bool]]:
    segments = list(prefix)
    if position < end and tokens[position].text == '::':
        segments.append('::')
        position += 1
    while position < end:
        text = tokens[position].text
        if text == '{':
            close = closing(tokens, position)
            start = position + 1
            while start < close:
                stop = separator(tokens, start, close, False)
                yield from use_tree(tokens, start, stop, tuple(segments))
                start = stop + 1
            return
        if text == '*':
            yield tuple(segments), None, True
            return
        segments.append(text)
        position += 1
        if position < end and tokens[position].text == '::':
            position += 1
            continue
        alias = tokens[position + 1].text if position < end and tokens[position].text == 'as' else None
        yield tuple(segments), alias, False
        return


def parse_type(
    module: Module,
    tokens: list[Token],
    position: int,
    public: bool,
    attributes: list[list[Token]],
) -> int:
    name = tokens[position + 1]
    header = after_generics(tokens, position + 2)
    body = find(tokens, header, BRACED_BODIES if tokens[header].text == 'where' else BODIES)
    close = body if tokens[body].text == ';' else closing(tokens, body)
    offenders: list[Offender] = []
    if tokens[position].text == 'enum':
        end = close + 1
        if public:
            offenders.extend(
                Offender(relative(module.file), variant.line, f'{name.text}::{variant.text}')
                for variant in struct_variants(tokens, body + 1, close)
            )
    else:
        end = find(tokens, close + 1, SEMICOLON) + 1 if tokens[body].text == '(' else close + 1
        exposed = public and body != close and has_public_field(tokens, body + 1, close)
        if exposed and not non_exhaustive(attributes):
            offenders.append(Offender(relative(module.file), name.line, name.text))
    module.bind(name.text, public, Item(tuple(offenders)))
    return end


def has_public_field(tokens: list[Token], position: int, end: int) -> bool:
    while position < end:
        attributes, position = read_attributes(tokens, position, end)
        public, position = read_visibility(tokens, position)
        if public and not test_only(attributes):
            return True
        position = separator(tokens, position, end, True) + 1
    return False


def struct_variants(tokens: list[Token], position: int, end: int) -> Iterator[Token]:
    while position < end:
        attributes, position = read_attributes(tokens, position, end)
        if position >= end:
            return
        _, position = read_visibility(tokens, position)
        if tokens[position + 1].text == '{' and not non_exhaustive(attributes) and not test_only(attributes):
            yield tokens[position]
        position = separator(tokens, position + 1, end, False) + 1


def resolve(module: Module, segments: tuple[str, ...], seen: frozenset[int]) -> list[Target]:
    if not segments:
        return []
    head = segments[0]
    if head == '::':
        return [External()]
    if head == 'crate':
        current: list[Target] = [module.root()]
    elif head == 'self':
        current = [module]
    elif head == 'super':
        current = [module.parent] if module.parent else []
    else:
        current = lookup(module, head, seen)
        if not current and head in module.external:
            return [External()]
    for segment in segments[1:]:
        following: list[Target] = []
        for target in current:
            if not isinstance(target, Module):
                following.append(target)
            elif segment == 'super':
                following.extend([target.parent] if target.parent else [])
            elif segment == 'self':
                following.append(target)
            else:
                following.extend(lookup(target, segment, seen))
        current = following
    return current


def lookup(module: Module, name: str, seen: frozenset[int]) -> list[Target]:
    targets: list[Target] = []
    for binding in module.bindings.get(name, ()):
        targets.extend(follow(binding.target, seen))
    if targets or id(module) in seen:
        return targets
    for glob in module.globs:
        for source in follow(glob.target, seen | {id(module)}):
            if isinstance(source, Module):
                targets.extend(lookup(source, name, seen | {id(module)}))
    return targets


def follow(target: Module | Item | External | Import | Alias, seen: frozenset[int]) -> list[Target]:
    if isinstance(target, Import):
        if id(target) in seen:
            return []
        return resolve(target.module, target.segments, seen | {id(target)})
    if isinstance(target, Alias):
        if id(target) in seen:
            return [TYPE_ALIAS]
        return [TYPE_ALIAS, *resolve(target.module, target.segments, seen | {id(target)})]
    return [target]


def reachable_offenders(root: Module) -> list[Offender]:
    offenders: list[Offender] = []
    unresolved: list[str] = []
    visited: set[int] = set()
    reached: set[int] = set()
    pending = [root]
    while pending:
        module = pending.pop()
        if id(module) in visited:
            continue
        visited.add(id(module))
        bindings = [binding for group in module.bindings.values() for binding in group] + module.globs
        for binding in bindings:
            if not binding.public:
                continue
            targets = follow(binding.target, frozenset())
            if not targets and isinstance(binding.target, Import):
                unresolved.append(binding.target.location)
            for target in targets:
                if isinstance(target, Module):
                    pending.append(target)
                elif isinstance(target, Item) and id(target) not in reached:
                    reached.add(id(target))
                    offenders.extend(target.offenders)
    if unresolved:
        raise AuditError(f'cannot resolve the `pub use` at {", ".join(sorted(unresolved))}')
    return offenders


def external_crates(manifest: Path) -> frozenset[str]:
    document = tomllib.loads(manifest.read_text())
    names = set(SYSROOT_CRATES)
    for table in [document, *document.get('target', {}).values()]:
        for section in ('dependencies', 'dev-dependencies', 'build-dependencies'):
            names.update(name.replace('-', '_') for name in table.get(section, {}))
    return frozenset(names)


def audit(crate: str) -> tuple[list[Offender], list[str]]:
    directory = CRATES / crate
    root_file = directory / 'src' / 'lib.rs'
    root = Module(root_file, root_file.parent, False, None, external_crates(directory / 'Cargo.toml'))
    parse_file(root)
    offenders = reachable_offenders(root)
    exhaustive = {(offender.path, offender.item) for offender in offenders}
    stale = [
        f'{path}: {item} is in ALLOWED but is no longer an exhaustive public type there'
        for path, item in sorted(ALLOWED)
        if path.startswith(f'crates/{crate}/') and (path, item) not in exhaustive
    ]
    return [offender for offender in offenders if (offender.path, offender.item) not in ALLOWED], stale


def crate_names(arguments: list[str], parser: argparse.ArgumentParser) -> list[str]:
    if not arguments:
        return sorted(path.name for path in CRATES.iterdir() if (path / 'src' / 'lib.rs').is_file())
    names = [Path(argument).name for argument in arguments]
    for name in names:
        if not (CRATES / name / 'src' / 'lib.rs').is_file():
            parser.error(f'no library crate named {name} under {relative(CRATES)}/')
    return names


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('crates', nargs='*', metavar='crate', help='a directory under crates/, such as abnegate-exec')
    names = crate_names(parser.parse_args().crates, parser)
    offenders: list[Offender] = []
    stale: list[str] = []
    try:
        for name in names:
            found, unneeded = audit(name)
            offenders.extend(found)
            stale.extend(unneeded)
    except AuditError as error:
        print(f'{Path(__file__).name}: {error}', file=sys.stderr)
        return 2
    for offender in sorted(offenders):
        print(offender)
    for entry in stale:
        print(entry)
    sys.stdout.flush()
    if offenders:
        print(
            f'{len(offenders)} public type(s) above can gain a field only in a breaking release: '
            'mark each #[non_exhaustive] (CONTRIBUTING.md, Public API).',
            file=sys.stderr,
        )
    return 1 if offenders or stale else 0


if __name__ == '__main__':
    sys.exit(main())
