//! Phase 1 spike: prove the two assumptions the whole architecture rests on.
//!
//! 1. Tiling via `set_origin` produces pixels identical to a full-page render.
//!    If this is wrong, the render layer would have to be built differently.
//! 2. A 512x512 tile rasterizes in roughly 6 ms, so a single serialized engine
//!    thread can keep up with scrolling.
//!
//! Run with: `cargo run --example tile_bench -p pdfreader-pdf -- <pdfium-dir> <pdf>`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pdfreader_core::Rotation;
use pdfreader_pdf::engine::PdfiumEngine;
use pdfreader_pdf::{DocumentHandle, PdfEngine, TileRequest};

/// Tile edge length used by the real renderer.
const TILE: u32 = 512;

fn main() {
    let mut args = std::env::args().skip(1);
    let pdfium_dir = PathBuf::from(args.next().expect("usage: tile_bench <pdfium-dir> <pdf>"));
    let pdf = PathBuf::from(args.next().expect("usage: tile_bench <pdfium-dir> <pdf>"));

    let mut engine = PdfiumEngine::bind(Some(&pdfium_dir)).expect("bind pdfium");

    let started = Instant::now();
    let (handle, info) = engine.open(&pdf, None).expect("open document");
    let open_time = started.elapsed();

    println!("file:      {}", pdf.display());
    println!("pages:     {}", info.page_count());
    println!("open:      {open_time:?}");

    if let Some(first) = info.pages.first() {
        println!(
            "page size: {:.2} x {:.2} pt",
            first.width_pt, first.height_pt
        );
    }

    verify_tiling(&mut engine, handle);
    bench_tiles(&mut engine, handle, &info);
    bench_text(&mut engine, handle, &info);

    engine.close(handle);
}

/// Render a page whole, then tile by tile, and compare the overlapping pixels.
fn verify_tiling(engine: &mut PdfiumEngine, handle: DocumentHandle) {
    let scale = 1.0;
    let page = 0u32;

    let full = engine
        .render_page(handle, page, Rotation::None, scale)
        .expect("full page render");

    let (full_w, full_h) = (full.width, full.height);
    println!("\n-- tiling correctness --");
    println!("full page render: {full_w} x {full_h} px");

    let mut compared = 0usize;
    let mut differing = 0usize;

    for ty in 0..full_h.div_ceil(TILE) {
        for tx in 0..full_w.div_ceil(TILE) {
            let origin_x = (tx * TILE) as i32;
            let origin_y = (ty * TILE) as i32;

            let tile = engine
                .render_tile(
                    handle,
                    &TileRequest {
                        page,
                        rotation: Rotation::None,
                        scale,
                        origin_x,
                        origin_y,
                        width: TILE,
                        height: TILE,
                    },
                )
                .expect("tile render");

            let overlap_w = (full_w.saturating_sub(origin_x as u32)).min(TILE) as usize;
            let overlap_h = (full_h.saturating_sub(origin_y as u32)).min(TILE) as usize;

            for y in 0..overlap_h {
                let full_row = (origin_y as usize + y) * full_w as usize;
                let tile_row = y * TILE as usize;

                for x in 0..overlap_w {
                    let fi = (full_row + origin_x as usize + x) * 4;
                    let ti = (tile_row + x) * 4;

                    compared += 4;
                    for c in 0..4 {
                        if full.data[fi + c] != tile.data[ti + c] {
                            differing += 1;
                        }
                    }
                }
            }
        }
    }

    let ratio = if compared == 0 {
        1.0
    } else {
        differing as f64 / compared as f64
    };

    println!("compared {compared} channel samples, {differing} differ ({ratio:.6})");
    println!(
        "{}",
        if ratio < 0.001 {
            "PASS: set_origin tiling matches a full-page render"
        } else {
            "FAIL: tiling does not reproduce the full-page render"
        }
    );
}

/// Measure per-tile rasterization cost across many pages.
fn bench_tiles(
    engine: &mut PdfiumEngine,
    handle: DocumentHandle,
    info: &pdfreader_pdf::DocumentInfo,
) {
    const SAMPLES: usize = 240;

    println!("\n-- tile raster cost ({SAMPLES} tiles, {TILE}x{TILE}) --");

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    let started = Instant::now();

    for i in 0..SAMPLES {
        let page = (i % info.page_count().max(1) as usize) as u32;

        let started_tile = Instant::now();
        let _ = engine.render_tile(
            handle,
            &TileRequest {
                page,
                rotation: Rotation::None,
                scale: 2.0,
                origin_x: 0,
                origin_y: 0,
                width: TILE,
                height: TILE,
            },
        );
        samples.push(started_tile.elapsed());
    }

    let wall = started.elapsed();
    samples.sort_unstable();

    let p50 = samples[samples.len() / 2];
    let p95 = samples[samples.len() * 95 / 100];
    let p99 = samples[samples.len() * 99 / 100];
    let mean = wall / SAMPLES as u32;

    println!("mean {mean:?}  p50 {p50:?}  p95 {p95:?}  p99 {p99:?}");
    println!(
        "throughput: {:.1} tiles/s on one serialized engine thread",
        SAMPLES as f64 / wall.as_secs_f64()
    );
}

/// Measure text extraction cost per page.
fn bench_text(
    engine: &mut PdfiumEngine,
    handle: DocumentHandle,
    info: &pdfreader_pdf::DocumentInfo,
) {
    let pages = info.page_count().min(50) as usize;

    println!("\n-- text extraction ({pages} pages) --");

    let started = Instant::now();
    let mut total_chars = 0usize;

    for page in 0..pages as u32 {
        if let Ok(text) = engine.page_text(handle, page) {
            total_chars += text.chars.len();
        }
    }

    let elapsed = started.elapsed();
    println!("{elapsed:?} total, {total_chars} chars");
    if pages > 0 {
        println!("{:?} per page", elapsed / pages as u32);
    }
}
