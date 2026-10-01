# abnegate-learn

In-memory trial memory. `Memory` records what was tried and how it turned out,
finds similar past trials by cosine similarity on caller-supplied embeddings,
and turns those neighbours into avoid/context/instruction suggestions the next
round can read.

Persistence and embedding stay in the application: implement [`Archive`] and
[`Embedder`] against the host store and model. This crate is the mechanism.

A host maps its own outcomes onto [`Verdict`], supplies embeddings, and reads
[`Digest`] entries or [`Advisor`] suggestions. Application types stay in the
host.

## Features

None.

## Usage

```sh
cargo add abnegate-learn
```

```rust
use abnegate_learn::{Memory, TrialInput, Verdict};

let mut memory = Memory::new();
memory.record(
    TrialInput::new("lab", "fuzz")
        .with_verdict(Verdict::Skip)
        .with_error("reprl-unavailable")
        .with_lesson("fuzz cannot run without reprl")
        .with_embedding(vec![1.0, 0.0, 0.0]),
);

let digest = memory.digest("lab", Some(&[0.99, 0.01, 0.0]));
assert!(!digest.failed_strategies.is_empty());
assert!(digest.as_prompt().contains("fuzz"));
```

`digest` is scoped. A host that runs several labs records each trial under that
lab's scope, then asks only for that scope before the next attempt, so one lab
sees its own failures and the other does not have to.

`entries` returns `(key, value)` rows a host can stamp onto the next attempt
(`learn_failed`, `learn_avoid_0`, `learn_cluster_0_strategy`, …). `as_prompt`
renders the same digest as text for a model.

Hydrate with [`Trial::from_input`](https://docs.rs/abnegate-learn/latest/abnegate_learn/struct.Trial.html#method.from_input)
and [`Memory::load`](https://docs.rs/abnegate-learn/latest/abnegate_learn/struct.Memory.html#method.load),
or implement `Archive` and call `Memory::restore`. Classify skip reasons with
`ErrorClass::classify`. Parse attempt logs with `Fingerprint::parse`, passing
the host's own action names.

## Moving from an in-house feedback module

- Outcomes are [`Verdict`] values (`Success`, `Partial`, `Failure`, `Skip`,
  `Empty`), not application enums such as merged/closed PRs.
- Embeddings are supplied on the trial, or through an [`Embedder`] the host
  implements. This crate does not load a model.
- There is no store. A host that wants durability implements [`Archive`] or
  hydrates [`Memory::load`] from its own database and records each new trial
  as it happens.
