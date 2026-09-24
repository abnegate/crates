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
- A public constant or static holds a slice (`&[&str]`), never a fixed-length
  array (`[&str; 4]`): adding an entry changes an array's type.
- Renaming a serialized field or variant keeps its wire name with
  `#[serde(rename = "...")]`, so NDJSON messages, saved agent sessions, agent
  configurations and provider request bodies written by an earlier version still
  read. A test pins the wire name.

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

## First publish

crates.io Trusted Publishing cannot create a crate name that does not yet exist,
so the first version of each crate is published by hand, from a machine with a
crates.io token. Until every crate has Trusted Publishing, `release-plz.toml`
sets `release = false` and `.github/workflows/release-plz.yml` runs only when
dispatched: with `publish` left off, it opens the release PR and never asks
crates.io for a token.

1. Pre-flight, on a clean `main` whose CI is green:

   ```sh
   git switch main && git pull --ff-only
   git status
   cargo publish --workspace --dry-run
   ```

2. Token. Sign in to crates.io with GitHub and verify the account's email
   address, which publishing requires. At
   <https://crates.io/settings/tokens/new> create a token named
   `abnegate-first-publish`, expiring in 7 days, with the scopes `publish-new`
   and `publish-update` and the crate pattern `abnegate-*`. In the publishing
   shell, either read it into the environment, which writes nothing to disk or
   history, or run `cargo login` now and `cargo logout` afterwards:

   ```sh
   read -rs CARGO_REGISTRY_TOKEN && export CARGO_REGISTRY_TOKEN
   ```

3. Publish in batches. crates.io lets new crates through in a burst of 5, then
   one every 10 minutes ([rate limits](https://crates.io/docs/rate-limits)),
   and `cargo publish --workspace` stops at the first 429. Either ask
   help@crates.io to raise the limit beforehand, or:

   ```sh
   cargo publish -p abnegate-secret -p abnegate-http -p abnegate-vision -p abnegate-exec -p abnegate-config
   for crate in abnegate-llm abnegate-notify abnegate-search abnegate-vcs abnegate-agent-cli abnegate-agent abnegate-comfy; do
     sleep 610
     cargo publish -p "$crate" || break
   done
   ```

   That takes about 75 minutes. After a 429, wait until the time the error
   names, then re-run from the crate that failed. Cargo waits for each
   dependency to reach the index before it publishes a dependent.

4. Verify:

   - `cargo search abnegate --limit 20` lists all 12 crates at 0.1.0.
   - Every docs.rs build at `https://docs.rs/crate/abnegate-<name>/0.1.0/builds`
     succeeded.
   - A scratch consumer builds against the published crates:

     ```sh
     cd "$(mktemp -d)" && cargo new --lib consumer && cd consumer
     cargo add abnegate-secret abnegate-llm abnegate-vcs --features abnegate-vcs/github
     cargo check
     ```

5. Tag the release, pushing only the 12 new tags. The names match
   release-plz's `git_tag_name`, so its first run starts from them:

   ```sh
   tags=()
   for crate in secret http vision exec config llm notify search vcs agent-cli agent comfy; do
     git tag "abnegate-$crate-v0.1.0"
     tags+=("abnegate-$crate-v0.1.0")
   done
   git push origin "${tags[@]}"
   ```

6. Afterwards:

   - Revoke the token.
   - On crates.io, open each crate's Settings, then Trusted Publishing, and add
     GitHub with owner `abnegate`, repository `crates`, workflow
     `release-plz.yml` and no environment.
   - In one change, lift the first-publish gates: drop `release = false` from
     `release-plz.toml`, run `release-plz.yml` on pushes to `main` as well as on
     dispatch, without the `publish` input, and drop `if: false` from the
     `semver` job in `ci.yml`.
   - Watch the first release-plz run that change triggers.
