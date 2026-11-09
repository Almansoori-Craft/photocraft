# epaint 0.37.0, storytold/egui@6d518fc, with the Al Majlis Arabic-joining patch

Vendored (`cargo package -p epaint` of storytold/egui at rev 6d518fc799681571963842f95b55a6b68431b15a)
so the patch builds without a fork of the egui repository. The only changes from that rev:

- `src/text/arabic_forms.rs` (new): contextual Arabic presentation forms and lam-alef.
- `src/text/text_layout.rs`, the `.notdef` fallback in `emit_cluster_glyphs`: a cluster drawn by a
  fallback GlyphRasterizer (the web build's Canvas 2D, used when no installed font has Arabic)
  gets its joining form; the alef of lam-alef is drawn as nothing.
- `src/text/mod.rs`: `mod arabic_forms;`.

Display only: text, glyph count and cursor positions do not change. Remove this folder and the
two `epaint` patch lines in the root Cargo.toml when upstream egui shapes the fallback itself.
