//! [`crate::ScrollView`]'s clip and scrollbar painters, re-exported at the
//! path pixel hosts import them from. Both live with the widget in
//! [`crate::primitives::scroll_view`], shared by every backend.

pub use crate::primitives::scroll_view::{draw_scrollbar_for, paint_clipped};
