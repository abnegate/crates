/// What a value read whole has to show of a credential before it counts as
/// one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Evidence {
    /// The prefix and enough after it, even when that reads as words, as
    /// `sk-learn-experiments` does.
    Prefix,
    /// A body that redaction takes out, so that `xai_list_models` stays a
    /// tool and `sk_bind_sendto_listen.c` a file.
    Body,
}
