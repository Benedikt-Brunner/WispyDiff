//! WispyDiff core: everything except the UI shell.

pub mod anchors;
pub mod assistant;
pub mod attribution;
pub mod cache;
pub mod diff;
pub mod drafts;
pub mod error;
pub mod git;
pub mod github;
pub mod highlight;
pub mod highlight_cache;
pub mod mcp;
pub mod inbox;
pub mod model;
pub mod noise;
pub mod pr_ref;
pub mod progress;
pub mod range;
pub mod repo_store;
pub mod review;
mod review_tools;
pub mod service;
pub mod shell_env;
pub mod split;
pub mod stack;
pub mod symbols;
pub mod token;
pub mod words;
pub mod wrap;

pub use error::{Error, Result};
