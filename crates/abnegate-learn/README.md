# abnegate-learn

In-memory trial memory. `Memory` records what was tried and how it turned out,
finds similar past trials by cosine similarity on caller-supplied embeddings,
and turns those neighbours into avoid/context suggestions the next round can
read. Persistence and embedding stay in the application: this crate is the
mechanism.

The donor is claudear's feedback loop (`FeedbackAnalyzer`, `OutcomeTracker`,
`LogExtractor`) with the issue/PR types left behind.

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

## Moving from an in-house feedback module

- Outcomes are [`Verdict`] values (`Success`, `Partial`, `Failure`, `Skip`,
  `Empty`), not application enums such as merged/closed PRs.
- Embeddings are supplied on the trial. This crate does not call an embedder.
- There is no store. A host that wants durability hydrates [`Memory::load`]
  from its own database and records each new trial as it happens.
