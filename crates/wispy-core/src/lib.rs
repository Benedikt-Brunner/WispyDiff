//! WispyDiff core: everything except the UI shell.

pub mod cache;
pub mod diff;
pub mod error;
pub mod git;
pub mod github;
pub mod highlight;
pub mod model;
pub mod pr_ref;
pub mod repo_store;
pub mod service;
pub mod token;

pub use error::{Error, Result};
