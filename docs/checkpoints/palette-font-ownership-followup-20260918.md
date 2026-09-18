# Palette and font ownership follow-up

Read-only navigator preparation for Stage 2.9. No implementation or runtime
failure reproduction is claimed; timer ownership remains the active item.

`player/bitmap/bitmap.rs` stores DEFAULT_SYSTEM_PALETTE in thread-local mutable
state. `DirPlayer::load_movie_from_dir_sync_with_options` sets it from the incoming movie's
platform before loading: platform 1 selects SystemMac; other values select
SystemWin. Production consumers include bitmap/color handlers, image creation,
font atlas construction, Canvas2D rendering and WebGL2 rendering. Loading a
second movie can therefore change the ambient palette observed by another
owner. This is a source-level dependency finding, not a reproduced pixel diff.

The same bitmap module stores four destination PaletteQuantizer instances in
thread-local DST_QUANTIZERS. A quantizer owns an exact resolved palette table
and a mutable RGB-to-index memo. Cache lookup compares the resolved table and
maintains most-recent-first eviction; the algorithm deliberately preserves exact
nearest-color answers. Do not replace it with approximation or discard the
multi-palette cache as an ownership shortcut.

CastManager already has an owner-local cached Rc<PaletteMap>, rebuilt through
`palettes()` and retired through `invalidate_palette_cache()`. PaletteMap is a
candidate carrier for explicit palette context and a local quantizer cache, but
the implementing lead must review its retained snapshot and invalidation lifetime
before choosing this placement. Pure immutable built-in color tables can remain
shared; a lexical static finding alone does not imply mutable ownership.

Proposed first bounded assignment: move movie default-palette selection and
destination quantizer caching into explicit owner/context state, migrate every
production consumer, and remove both thread-local accessors. Preserve platform
selection, custom palette fallback/order, original bit-depth handling, exact
quantization and cache invalidation. Required proof includes interleaved owners
with distinct Mac/Windows defaults and overlapping cast IDs, load/reset of one
owner without changing the neighbor, custom-palette mutation, and representative
indexed copyPixels plus actual Canvas2D/WebGL2 rendering where those paths use
the default. Native pure pixel tests must verify exact results; browser tests
must exercise exported owner entrypoints. This is ownership migration, not the
Stage 4 deterministic renderer project.

Separate font assignment: `player/font/mod.rs` stores GLYPH_PREFERENCE in a
thread-local Cell although FontManager and its caches already belong to a
player. `lib.rs` exposes global set/get preference and ambient clear_font_cache;
the preference is consumed in text handlers, WebGL2 rendering and PFR rasterizer
entrypoints. Move the preference and public controls to the explicit owner,
pass rasterization options through the font-building call chain, and preserve
the current mode semantics (auto/bitmap/native/outline, with explicit cache
clear where currently required). Prove two owners with different modes and
independent cache reset, including a late font completion. Do not combine this
with the palette assignment merely because both contribute to rendering.

Potential overlap with future audio/input/timer work is limited chiefly to
lib.rs and player/mod.rs. Serialize those edits under a mandatory Sol lead and
Luna pilot; root owns this preparation document. No new assignment is active.
