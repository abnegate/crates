//! A [`SecretValue`](crate::SecretValue) reads and writes as the text column it
//! is stored in.

#[cfg(feature = "rusqlite")]
#[cfg_attr(docsrs, doc(cfg(feature = "rusqlite")))]
mod rusqlite;

#[cfg(feature = "sqlx")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlx")))]
mod sqlx;
