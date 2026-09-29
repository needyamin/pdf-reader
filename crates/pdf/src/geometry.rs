//! Axis-aligned rectangles in PDF page space.
//!
//! The type itself lives in `pdfreader_core` so that the engine, the renderer
//! and the domain state all agree on which way up a rectangle is — core cannot
//! depend on this crate, and form fields and annotations have to live in core
//! state so the UI can read them. It is re-exported here because the engine's
//! public API speaks in these rects.

pub use pdfreader_core::Rect;
