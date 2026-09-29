//! Fuzz [`quadraui::compose::markdown::render_markdown_to_styled`]
//! (quadraui#1130).
//!
//! Untrusted input: a markdown document reaches this from chat
//! transcripts, README files, or AI-generated output — none of which
//! this crate controls the shape of. `src/compose/markdown.rs::proptests`
//! covers the same "never panics" + "line vectors stay aligned" + "link
//! byte-ranges stay valid" properties with `proptest`'s bounded per-run
//! case count; this target is the same contract under continuous,
//! coverage-guided fuzzing (see `.github/workflows/fuzz.yml`).
#![no_main]

use libfuzzer_sys::fuzz_target;
use quadraui::Theme;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let theme = Theme::default();
    let r = quadraui::compose::markdown::render_markdown_to_styled(&s, &theme);

    // Cheap invariant, worth checking on every corpus entry: the
    // length-aligned vectors this module's doc promises must actually
    // stay aligned (see `RenderedMarkdown`'s "Side-channels" doc).
    assert_eq!(r.lines.len(), r.line_text.len());
    assert_eq!(r.lines.len(), r.line_scales.len());

    // Every recorded link's byte range must be genuinely valid into
    // `line_text` — in bounds and on char boundaries — or a consumer
    // slicing `line_text[line_idx][range]` panics on whatever
    // pathological input first breaks that contract.
    for (line_idx, range, _url) in &r.links {
        assert!(*line_idx < r.line_text.len());
        let line = &r.line_text[*line_idx];
        assert!(range.start <= range.end);
        assert!(range.end <= line.len());
        assert!(line.is_char_boundary(range.start));
        assert!(line.is_char_boundary(range.end));
    }
});
