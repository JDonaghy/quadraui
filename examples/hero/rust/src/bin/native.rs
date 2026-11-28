//! The hero counter in a native window: the identical `Counter` from
//! `src/main.rs`, handed to the native runner for whichever backend
//! feature is enabled (`--features tui,macos`, `tui,gtk` or `tui,win`).

#[allow(dead_code)]
#[path = "../main.rs"]
mod hero;

#[allow(unreachable_code)]
fn main() -> Result<(), quadraui::RunError> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    return quadraui::macos::run(hero::Counter::default());
    #[cfg(feature = "gtk")]
    return quadraui::gtk::run(hero::Counter::default());
    #[cfg(feature = "win")]
    return quadraui::win::run(hero::Counter::default());
    panic!("build with one native backend feature: macos, gtk or win");
}
