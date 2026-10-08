//! The demo registry — the one list port issues append a line to.
//!
//! Each entry is a constructor for one boxed [`crate::Demo`]. The gallery
//! shell (`crate::app::GalleryApp`) only ever iterates this list; it has
//! no per-demo knowledge beyond what `Demo` exposes, so adding a demo is
//! exactly one line here plus the demo's own module — no shell code
//! changes (issue #1341's "registry" requirement).

use crate::demos::toast::ToastDemo;
use crate::Demo;

/// Construct every registered demo, in registration order.
///
/// Port issues append one `Box::new(...)` line to this `vec!` — nothing
/// else in the gallery needs to change. The registry-driven test in
/// `tests/gallery_driver.rs` exercises every entry this returns, so a
/// newly-appended demo gets smoke coverage for free.
pub fn registry() -> Vec<Box<dyn Demo>> {
    vec![Box::new(ToastDemo::new())]
}
