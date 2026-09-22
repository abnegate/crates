# Contributing

## Donor and absorb

Every crate starts from one named donor implementation — the most complete of
the two or three that already exist — and absorbs the unique capabilities of the
others before its first release. Where the donors disagree, the donor's public
names win and the absorbed code is adapted to them.

Application-specific types never cross. A crate here carries the mechanism, not
one application's domain model: no application enums, no application error
variants, no configuration structs that name an application's settings file. If
a capability cannot be expressed without such a type, it stays in the
application.

## Comments

Default to none. Code is made self-explanatory through clear names, small
functions, and typed values. If a comment is needed to explain *what* the code
does, the code is wrong — fix the code. Rationale, tradeoffs, and the reason
behind a non-obvious value belong in the commit message.

The exceptions are an invariant a reader would otherwise violate, a workaround
for an external bug, and a deliberately empty block. Never write section-header
comments (`// ---`, `// ===`); a file that needs them is a file that should be
split.

Doc comments are not narration and are not covered by this rule: every public
item carries one.

## Naming

- Single-word names where context makes the meaning obvious: `connections`, not
  `backendConnections`; `timeout`, not `connectTimeout` when it is the only one.
- Never abbreviate: `certificate` not `cert`, `connection` not `conn`,
  `request` not `req`. Well-known acronyms (TLS, HTTP, TCP, CA) are fine.
- Acronyms in function names are camelCase-equivalent in snake_case:
  `update_mfa()`, not `update_m_f_a()`. In constants, capitalise fully.
- One type per file, and the file is named for the type. Modules never repeat
  their parent's name: `secret/value.rs`, not `secret/secret_value.rs`.
- Feature names are kebab-case.
- Full type annotations on every signature.

## Edition and MSRV

Every crate is edition 2024 with `rust-version = "1.97"`. Edition is per-crate,
so an edition 2021 consumer links these without changing its own. The MSRV is
the real floor and the `msrv` CI job holds it: raising it is a minor version
bump and needs a reason in the commit message.

`rust-toolchain.toml` pins the development toolchain at 1.98, which is not the
MSRV — it is what contributors and CI build with.

## Dependencies

- Caret ranges only. Never `=`-pin: a library that pins exactly collides with
  every consumer's resolution. The one exception is a pre-release, where an rc
  bump is breaking and the range is written explicitly.
- New dependencies go in `[workspace.dependencies]` at the root with their
  feature set, and crates take them with `dep.workspace = true`. A crate may add
  features on top; it may not change the version.
- Anything heavy — a native library, an ONNX runtime, a database driver — goes
  behind a feature, and `default = []`.
- `cargo hack check --workspace --feature-powerset --depth 2` has to pass, so
  every feature combination must compile on its own.

## Commits

Conventional commits: `type(scope): subject`, where type is one of `feat`,
`fix`, `refactor`, `chore`, `docs`, `test`, `style`, `perf`. The subject says
why, not what — the diff already says what. release-plz reads these to decide
each crate's next version.

## First publish

crates.io Trusted Publishing cannot create a crate name that does not yet exist.
The first version of each crate is therefore published by hand, from a machine
with a crates.io token:

```sh
cargo login
cargo publish -p abnegate-secret
```

Trusted Publishing is configured per crate on crates.io once that first version
exists. Only then are `release = false` in `release-plz.toml` and the
`workflow_dispatch`-only trigger on `.github/workflows/release-plz.yml` lifted.
