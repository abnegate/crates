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
- That includes `max`, `min`, `env`, `dir`, `repo`, `pr`, `auth`, `spec`,
  `info`, `ext`, `hex`, `chars`, `secs`, `ms`, `len` in constant names,
  `img2img`, `i2v`, `t2v`, `Ack` and `Rect`, in private names as much as public
  ones: `MAXIMUM_REDIRECTS`, `working_directory`, `from_hexadecimal`,
  `image_to_video`, `HelloAcknowledged`. Environment variable names are spelled
  out the same way (`COMFYUI_MODELS_DIRECTORY`), with no fallback to an older
  name. The exceptions are Rust's own idioms (`as_str`, `len()`, `from_str`, a
  log level named `Info`), acronyms (`fps`, `url`, `id`, `sha`, `usd`, `vram`,
  `mime`, `ai`, `cli`, `mcp`, `http`, `tz`), proper names (`OAuth`) and wire
  strings.
- A timeout, time limit or interval is a `Duration`, never an integer with its
  unit in the name: `timeout: Duration`, not `timeout_ms: u64`.
- The one exception is a measured or requested media length on a serialized
  modality wire type, which stays `f64` seconds: `duration_seconds` on
  `MusicRequest`, `SoundEffectRequest`, `VideoRequest`, `AudioResponse`,
  `VideoResponse` and `TranscriptionResponse`, and `TranscriptionSegment`'s
  `start` and `end`. These are lengths of audio or video, not time limits, and
  vendors send and read them as fractional seconds, so a `Duration` would
  change the wire shape and gain nothing. A limit on them is still a
  `Duration`: `AudioProvider::maximum_duration()`.
- The crate-wide error is `Error`: `abnegate_http::Error`, not `HttpError`.
  Every other error is `<Domain>Error`, and the domain never repeats the crate
  name: `abnegate_vision::CropError`, not `abnegate_vision::crop::Error`. No
  type named bare `Error` lives below the crate root.
- Acronyms in function names are camelCase-equivalent in snake_case:
  `update_mfa()`, not `update_m_f_a()`. In constants, capitalise fully.
- One type per file, and the file is named for the type. Modules never repeat
  their parent's name: `secret/value.rs`, not `secret/secret_value.rs`.
- Feature names are kebab-case. A feature that exposes test doubles is named
  `testing`, in every crate.
- Full type annotations on every signature.

## Public API

Every crate is shaped so that it can grow without a breaking release:

- Every public enum is `#[non_exhaustive]`, so adding a variant is never a
  breaking change.
- Every public struct with public fields is `#[non_exhaustive]`, so adding a
  field is not one either. A struct literal then no longer compiles outside the
  crate, so every struct a caller supplies — a function argument, a trait
  method's return value, or configuration — has a constructor, or `Default` plus
  `with_*` methods. Closed geometry whose fields are the whole type, such as
  `Point` and `Rectangle`, stays exhaustive.
- Every struct variant of a public enum is `#[non_exhaustive]` too, so it can
  gain a field the same way. Outside the crate it then cannot be built with a
  literal, and a pattern that matches it ends in `..`, so a variant a caller
  builds, such as an error a `Notifier` implementation returns, has a
  constructor.
- A public constant or static holds a slice (`&[&str]`), never a fixed-length
  array (`[&str; 4]`): adding an entry changes an array's type.
- Renaming a serialized field or variant keeps its wire name with
  `#[serde(rename = "...")]`, so NDJSON messages, saved agent sessions, agent
  configurations and provider request bodies written by an earlier version still
  read. A test pins the wire name.

`scripts/audit_public_api.py` enforces the `#[non_exhaustive]` rules for
structs and struct variants, and CI runs it. It needs Python 3.9 or later and
nothing outside the standard library. With no argument it audits every crate;
given crate names, it audits only those:

```sh
python3 scripts/audit_public_api.py
python3 scripts/audit_public_api.py abnegate-exec abnegate-http
```

It follows each crate's modules down from `src/lib.rs` and audits only what a
caller can reach: code compiled only for tests, items that are not `pub`,
`pub` items of a private module that nothing re-exports, and re-exports of a
path outside the crate, such as `pub use Option::Some`, are skipped. Each
public struct with a public field, and each struct variant of a public enum,
that is not `#[non_exhaustive]` is printed as `path:line: item`. An item that
stays exhaustive by ruling, such as `Point` and `Rectangle`, is listed in the
script's `ALLOWED`.

The exit status says which of three things happened:

- 0: every public type the audit reached can gain a field.
- 1: it printed at least one that cannot, or an `ALLOWED` entry that no longer
  names an exhaustive type.
- 2: the audit could not finish, so it proves nothing either way: a crate
  named on the command line does not exist, a module file is missing, a path
  that enters a crate names something the audit cannot find, a source cannot
  be parsed, the script itself failed, or Python is older than 3.9.

The script's own tests run the audit over a crate written for each case, and
CI runs them too:

```sh
python3 -m unittest discover --start-directory scripts
```

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
- New dependencies go in `[workspace.dependencies]` at the root, and crates take
  them with `dep.workspace = true`. A crate never changes the version.
- Features are declared by the crate that uses them. The root entry carries the
  version, turns default features off where they are heavy, and names only the
  features every crate taking the dependency needs. Everything else is declared
  where it is used: in `[dependencies]` for library code, in
  `[dev-dependencies]` for what only tests need (`tokio`'s `rt-multi-thread` and
  `test-util`), and in a crate feature for what only that feature's code needs
  (`openai = ["dep:base64", "reqwest/multipart", "tokio/fs"]`). A consumer then
  compiles only what the crates it uses need.
- Anything heavy — a native library, an ONNX runtime, a database driver — goes
  behind a feature, and `default = []`.
- Every feature combination, with and without its tests, has to compile and
  lint clean on its own:

  ```sh
  cargo hack check --workspace --feature-powerset --depth 2 --all-targets
  cargo hack check --workspace --feature-powerset --depth 2 --no-dev-deps
  cargo hack clippy --workspace --each-feature --all-targets -- -D warnings
  ```

  The `--no-dev-deps` run is the one that catches a missing declaration: with
  `--all-targets`, a dev-dependency's features reach the library build too.

## Documentation

docs.rs builds with every feature, but a consumer running `cargo doc` gets only
the features it enabled, so an intra-doc link to a feature-gated item has to
resolve without that feature too. Both builds have to pass:

```sh
RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo +nightly doc --workspace --all-features --no-deps
RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo +nightly doc --workspace --no-default-features --no-deps
```

## Security posture

Every crate is safe by default, and a change that weakens any of the following
needs a reason in the commit message. Child processes receive an allowlisted
environment by default, never the parent's whole environment. All outbound HTTP
validates its target before connecting and refuses redirects that cross
origins. Every error type is redacted, so an error that reaches a log never
carries a secret.

## Tests

Every behaviour change and every bug fix comes with a regression test that
fails without it.

Tests never call `std::env::set_var` or `remove_var`: both are `unsafe` in
edition 2024, and the crates deny unsafe code. A test that needs a variable set
hands it to a child instead, either on the `Command` it spawns or by re-running
its own test binary with the variable set (the `ABNEGATE_EXEC_TEST_CHILD`
pattern in abnegate-exec).

## Commits

Conventional commits: `type(scope): subject`, where type is one of `feat`,
`fix`, `refactor`, `chore`, `docs`, `test`, `style`, `perf`. The subject says
why, not what — the diff already says what. release-plz reads these to decide
each crate's next version.

## Releases

`.github/workflows/release-plz.yml` runs release-plz on every push to `main`.
Its release PR job opens or updates a single release pull request, which bumps
versions from the conventional commits since each crate's last release tag,
checks with cargo-semver-checks that each bump is large enough for the API
change, and updates each crate's `CHANGELOG.md`. Nothing is published until
that pull request is merged.

Merging the release pull request publishes every crate whose new version is not
yet on crates.io. The release job authenticates through crates.io Trusted
Publishing: release-plz exchanges the job's GitHub OIDC token for a short-lived
publish token scoped to this repository and `release-plz.yml`, and revokes it
once the job has published. No crates.io token is stored anywhere. Each crate's
Trusted Publishing settings on crates.io name GitHub, owner `abnegate`,
repository `crates`, workflow `release-plz.yml` and no environment. Trusted
Publishing cannot create a crate, so a new crate's first version is published
by hand and its Trusted Publishing entry added before release-plz can publish
it.

Every release is tagged `abnegate-<name>/v<version>`, such as
`abnegate-http/v0.2.0`, and gets a GitHub release of the same name from its
changelog entry. A crate whose tag already exists counts as released.

Each crate carries its own `version` rather than inheriting one from the
workspace, so release-plz bumps and releases only the crates that changed, and
the crates that depend on them. A shared workspace version would move every
crate that inherits it whenever any of them is released.

Feature pull requests are not semver-checked in CI: versions move only in the
release pull request, where release-plz has already run cargo-semver-checks to
choose each bump. The `semver` CI job runs only on the release pull request,
whose branch starts with `release-plz-`, and runs `cargo semver-checks
check-release` against each crate's latest version on crates.io to confirm the
proposed bump is big enough. cargo-semver-checks refuses a crate that has never
been published, so a new crate's first version reaches crates.io before the
release pull request that includes it can pass.

release-plz opens and updates the release pull request with the workflow's own
`GITHUB_TOKEN`, and a pull request or push made with `GITHUB_TOKEN` triggers no
workflows. A `workflow_dispatch` is the exception, so once release-plz has
opened or updated the pull request, the release PR job dispatches `ci.yml` on
its `release-plz-` branch through `gh workflow run`, which is why that job alone
holds `actions: write`. The dispatched run checks every crate and runs the
`semver` job, and its checks attach to the branch's head commit, so they show on
the release pull request. No personal access token or GitHub App is involved.
If required status checks are ever turned on for `main`, the release PR job
needs a GitHub App token or personal access token in place of `GITHUB_TOKEN`,
because checks from a dispatched run do not count toward required checks from
a `pull_request` run. The release PR job cannot open the pull request without
the repository setting "Allow GitHub Actions to create and approve pull
requests", under Settings, Actions, General, Workflow permissions. It is
required, and it is on.
