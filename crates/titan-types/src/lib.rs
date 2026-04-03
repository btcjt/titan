//! Core types for the Titan nsite browser.

pub mod name;
pub mod url;
pub mod error;

pub use name::{TitanName, TitanOp, OpAction};
pub use url::NsiteUrl;
pub use error::TitanError;
