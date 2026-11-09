//! A [`SecretValue`](crate::SecretValue) reads and writes as the text column it
//! is stored in.

#[cfg(feature = "sqlx")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlx")))]
mod sqlx;
