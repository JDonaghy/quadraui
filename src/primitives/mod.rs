//! Widget primitives.
//!
//! Each primitive module exports a declarative data struct describing the
//! widget, a companion event enum, and any supporting types. Backends
//! implement rendering and input handling against these types.

pub mod form;
pub mod palette;
pub mod tree;
