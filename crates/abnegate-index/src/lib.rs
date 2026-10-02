#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Overlapping-window source chunking and host-owned semantic code search.
//!
//! [`index_tree`] walks a directory, splits each file with [`chunks`], and
//! writes windows through a [`Store`] the host implements. Embeddings are
//! optional: pass an [`Embedder`] to attach vectors, or `None` so later
//! keyword search still works. [`Memory`] is an in-memory store; persistence
//! and models stay in the host. This crate never opens a database or loads a
//! model.
//!
//! ```
//! use abnegate_index::{Hit, Memory, Method, Store};
//!
//! let store = Memory::new();
//! store
//!     .replace_file(
//!         "/src",
//!         "api.rb",
//!         "rb",
//!         "abc",
//!         &[("get ':id/entities'".into(), Some(vec![1.0, 0.0]))],
//!     )
//!     .unwrap();
//!
//! let hits: Vec<Hit> = store.search_vector(&[1.0, 0.0], Some("/src"), 4).unwrap();
//! assert_eq!(hits[0].relative, "api.rb");
//! assert_eq!(hits[0].method, Method::Cosine);
//! ```

mod cancel;
mod chunk;
mod embedder;
mod error;
mod generated;
mod hash;
mod hit;
mod index;
mod language;
mod memory;
mod method;
mod proceed;
mod relative;
mod similarity;
mod stats;
mod store;
mod walk;

#[cfg(any(test, feature = "testing"))]
mod scripted;

pub use crate::cancel::Cancel;
pub use crate::chunk::OVERLAP;
pub use crate::chunk::SMALLEST;
pub use crate::chunk::WINDOW;
pub use crate::chunk::chunks;
pub use crate::embedder::Embedder;
pub use crate::error::Error;
pub use crate::error::Result;
pub use crate::generated::generated;
pub use crate::hash::file_hash;
pub use crate::hit::Hit;
pub use crate::index::EMBED_BATCH;
pub use crate::index::index_texts;
pub use crate::index::index_tree;
pub use crate::index::index_walk;
pub use crate::language::language_of;
pub use crate::memory::Memory;
pub use crate::method::Method;
pub use crate::proceed::Proceed;
pub use crate::relative::relative_to;
pub use crate::relative::root_key;
pub use crate::similarity::SIMILARITY_FLOOR;
pub use crate::similarity::cosine_similarity;
pub use crate::stats::Stats;
pub use crate::store::Store;
pub use crate::walk::EXTENSIONS;
pub use crate::walk::LARGEST_FILE;
pub use crate::walk::SKIP_DIRECTORIES;
pub use crate::walk::Walk;
pub use crate::walk::walk_source;

#[cfg(feature = "testing")]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub use crate::scripted::Scripted;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
