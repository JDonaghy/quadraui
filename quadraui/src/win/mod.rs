//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! Scaffolded module structure — every Backend trait method has a
//! `todo!()` stub. Implement each one against Direct2D / DirectWrite
//! and the compiler will tell you when you're done.
//!
//! See `quadraui/docs/NATIVE_GUI_LESSONS.md` for pitfalls discovered
//! during earlier Win-GUI work. See the GTK backend (`quadraui/src/gtk/`)
//! as the reference implementation for a pixel-based backend.

pub mod backend;
pub mod run;
pub mod services;

pub use backend::WinBackend;
pub use run::run;
pub use services::WinPlatformServices;
