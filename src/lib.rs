//! SolidCraft importer plugin for Autodesk Inventor parts (.ipt).
//!
//! The plugin decodes the part's B-rep with cadmpeg's Inventor codec, writes it as STEP AP214
//! (as SolidCraft's own FreeCAD/Rhino route does in crates/io native_cad::convert::step_route),
//! and returns a design whose one Import feature carries that STEP text inline. SolidCraft's
//! native STEP import then builds the solid when the design opens.

wit_bindgen::generate!({ path: "wit", world: "importer-plugin" });

use std::collections::BTreeMap;
use std::io::Cursor;

use cadmpeg_codec_inventor::InventorCodec;
use cadmpeg_core::decode::{DecodeArena, DecodeContext, DecodeDepth, DecodeMode, DecodePolicy, ResourceLimits};
use cadmpeg_ir::codec::write::{EncodeInput, target::TargetRequest};
use cadmpeg_ir::validate::validate_neutral_for_decode;
use cadmpeg_ir::{Codec, DecodeOptions};
use cadmpeg_registry::{Format, build_encoder};
use craft::plugin::types::{FieldFailure, PluginError};
use craft::plugin::{control, design, log, source};
use exports::craft::plugin::importer::Guest;
use serde_json::{Value, json};

struct Importer;

impl Guest for Importer {
    fn import_file(_entry: String, file: source::FileInfo, _options: String) -> Result<String, PluginError> {
        let bytes = read_all(&file)?;
        let stem = file.name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&file.name).to_string();
        let name = if stem.trim().is_empty() { "Part".to_string() } else { stem };

        let (step, mut items, faces) = ipt_to_step(&bytes, &file.name, &name)?;

        let made = json!({
            "format": "solidcraft/57", "name": "", "units": "mm", "params": [],
            "features": [{
                "id": 1, "name": name, "type": "import", "suppressed": false,
                "file": file.name, "body_names": [name], "step": step,
            }],
            "next_id": 2, "reserved_body_names": [name], "next_component_color_index": 0
        });
        for chunk in made.to_string().as_bytes().chunks(1 << 20) {
            design::write(chunk)?;
        }

        items.sort_by_key(|i| severity_rank(i["severity"].as_str().unwrap_or("")));
        let summary = format!("Inventor part {} read with cadmpeg: {} face(s) as STEP", file.name, faces);
        Ok(json!({"summary": summary, "items": items,
                  "details": {"decoder": "cadmpeg-codec-inventor 0.6.12", "faces": faces}}).to_string())
    }
}

fn read_all(file: &source::FileInfo) -> Result<Vec<u8>, PluginError> {
    let mut bytes = Vec::with_capacity(file.size as usize);
    while (bytes.len() as u64) < file.size {
        let chunk = source::read("", bytes.len() as u64, 1 << 20)?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Decode the .ipt and write it as STEP. Returns the STEP text, the loss items and the face count.
fn ipt_to_step(bytes: &[u8], file: &str, name: &str) -> Result<(String, Vec<Value>, usize), PluginError> {
    if !bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Err(invalid(format!("{file} is not an Inventor file (no compound-file header)")));
    }
    let policy = DecodePolicy { mode: DecodeMode::Salvage, limits: ResourceLimits::service() };
    let options = DecodeOptions { depth: DecodeDepth::Full, policy: policy.clone() };
    log::log(log::Level::Info, &format!("decoding {file} ({} bytes)", bytes.len()));
    control::progress(0.1, "Decoding the Inventor part");
    let decoded = InventorCodec
        .decode(&mut Cursor::new(bytes), &options)
        .map_err(|e| invalid(format!("could not read {file}: {e}")))?;
    let (ir, report, fidelity) = decoded.into_parts();
    if !report.geometry_transferred() {
        return Err(PluginError::Unsupported(format!(
            "{file}: no solid geometry could be read (an assembly, or a part kernel cadmpeg does not decode yet)"
        )));
    }
    let faces = ir.model.faces.len();

    // Group cadmpeg's losses by code, so the report shows one line per kind.
    let mut grouped: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for l in &report.losses {
        let sev = match format!("{:?}", l.severity).to_ascii_lowercase().as_str() {
            s if s.contains("info") || s.contains("note") => "info",
            s if s.contains("error") || s.contains("block") || s.contains("fatal") => "dropped",
            _ => "degraded",
        }
        .to_string();
        let e = grouped.entry(l.code.to_string()).or_insert((sev, l.message.clone(), 0));
        e.2 += 1;
    }

    // Validate the neutral model, as cadmpeg's own conversion does.
    let arena = DecodeArena::new();
    let ctx = DecodeContext::for_loaded_input(&arena, &policy, bytes.len() as u64).map_err(|e| trap(e))?;
    let validation = validate_neutral_for_decode(&ctx, &ir, Vec::new()).map_err(|e| trap(e))?;
    ctx.finish_session().map_err(|e| trap(e))?;
    if !validation.is_ok() {
        return Err(invalid(format!("{file}: the decoded model is invalid ({} errors)", validation.error_count())));
    }

    if control::cancelled() {
        return Err(PluginError::Cancelled);
    }
    control::progress(0.6, "Writing STEP");
    let plan = build_encoder(Format::Step)
        .plan(EncodeInput::new(&ir, Some(&fidelity)), TargetRequest::Explicit("ap214"))
        .map_err(|e| invalid(format!("{file}: could not restate as STEP: {e}")))?;
    for l in &plan.report().losses {
        if l.code.to_string() == "step/export.no-exportable-solids" {
            return Err(PluginError::Unsupported(format!("{file}: no solids that can be imported")));
        }
        let e = grouped.entry(l.code.to_string()).or_insert(("info".into(), l.message.clone(), 0));
        e.2 += 1;
    }
    let mut out = Vec::new();
    plan.write_to(&mut out).map_err(|e| trap(e))?;
    let mut step = String::from_utf8(out).map_err(|e| trap(e))?;
    // cadmpeg names a product it has no name for `cadmpeg_model`; name it after the file.
    if step.contains("'cadmpeg_model'") {
        step = step.replace("'cadmpeg_model'", &format!("'{}'", name.replace('\'', "''")));
    }

    // The host takes at most 4096 printable items: message <= 1024 chars, subject <= 256,
    // title <= 120, code <= 128.
    let items = grouped
        .into_iter()
        .take(4096)
        .map(|(code, (severity, message, n))| {
            json!({"severity": severity, "title": clamp(title_for(&code), 120),
                   "subject": if n > 1 { format!("{n} times") } else { String::new() },
                   "message": clamp(&message, 1024), "code": clamp(&code, 128)})
        })
        .collect();
    Ok((step, items, faces))
}

/// The kind of loss in the user's words, one per family of cadmpeg codes.
fn title_for(code: &str) -> &'static str {
    match code.split(['/', '.']).nth(1).unwrap_or("") {
        "geometry" => "Geometry that could not be read",
        "feature" => "Feature history kept out (the shape is imported, not the features)",
        "sketch" => "Sketches kept out",
        "appearance" => "Colours and appearances",
        "metadata" => "File properties",
        "pcurve" | "export" => "STEP details",
        _ => "Parts of the file kept as they are",
    }
}

/// Printable text of at most `max` characters: control characters become spaces, and a cut
/// message ends with an ellipsis.
fn clamp(s: &str, max: usize) -> String {
    let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if clean.chars().count() <= max {
        clean
    } else {
        let mut cut: String = clean.chars().take(max - 1).collect();
        cut.push('…');
        cut
    }
}

fn severity_rank(s: &str) -> u8 {
    match s { "dropped" => 0, "degraded" => 1, _ => 2 }
}

fn invalid(reason: String) -> PluginError {
    PluginError::InvalidInput(FieldFailure { field: "file".into(), reason })
}

fn trap(e: impl ToString) -> PluginError {
    PluginError::Trap(e.to_string())
}

export!(Importer);

/// getrandom's custom backend (selected in .cargo/config.toml). The host offers no entropy, and
/// nothing on the read path needs it: zip draws random bytes only to write encrypted archives.
/// A deterministic SplitMix64 stream keeps any caller working rather than failing.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), getrandom::Error> {
    static STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    for i in 0..len {
        let mut z = STATE.fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::Relaxed);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        unsafe { *dest.add(i) = (z ^ (z >> 31)) as u8 };
    }
    Ok(())
}
