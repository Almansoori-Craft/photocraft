//! Al Majlis fork (Studio 2c, 2026-11-12): memory-only PNG handoff for photo (raster) smart
//! objects, the raster twin of [`crate::smart_svg_cmds`]. `exportPng` hands out the contents as a
//! PNG at their own pixel size with an optimistic identity; `updatePng` swaps in an edited PNG
//! only while that identity still holds, keeping every instance's placement. No file system: the
//! web build has none, which is why `exportContents` / `replaceContents` (path based) can't serve.

use base64::Engine as _;
use std::sync::Arc;

use photocraft_doc::{Document, Layer, LayerContent, LayerId, SmartContentsId, SmartObject, SmartSource};
use photocraft_geom::{Affine, Rect};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

pub const EXPORT: &str = "layer.smartObjects.exportPng";
pub const UPDATE: &str = "layer.smartObjects.updatePng";
/// Largest PNG either way (bytes, before base64).
pub const MAX_PNG_BYTES: usize = 64 << 20;
const MAX_PIXELS: u64 = 24_000_000;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

/// The `layer` param; updates must name it, exports fall back to the active layer.
fn layer(s: &Session, p: &Value, cmd: &str, required: bool) -> Result<LayerId> {
    if !p.is_object() {
        return Err(bad(cmd, "parameters must be an object"));
    }
    match p.get("layer") {
        Some(v) => v.as_u64().map(LayerId).ok_or_else(|| bad(cmd, "`layer` must be an integer layer id")),
        None if required => Err(bad(cmd, "missing `layer`")),
        None => crate::commands::layer_param(s, p),
    }
}

fn smart(doc: &Document, id: LayerId) -> Result<&SmartObject> {
    match &doc.layer(id).ok_or(EngineError::NoLayer(id))?.content {
        LayerContent::Smart(sm) => Ok(sm),
        _ => Err(other("the layer is not a smart object")),
    }
}

/// True for SVG, PDF, EPS and PDF-compatible AI sources: those go through `exportSvg`.
fn is_vector(bytes: &[u8]) -> bool {
    photocraft_io::svg::is_svg(bytes)
        || photocraft_io::vector_preview::is_native_source(bytes)
        || bytes.starts_with(b"%PDF-")
        || bytes.starts_with(b"%!PS-Adobe-")
        || bytes.starts_with(&[0xc5, 0xd0, 0xd3, 0xc6])
}

/// The embedded raster contents (name, bytes) without following a linked disk path.
fn embedded<'a>(doc: &'a Document, sm: &'a SmartObject) -> Result<(String, &'a [u8])> {
    let (name, bytes) = match &sm.source {
        SmartSource::Embedded { file_name, bytes } => (file_name.clone(), bytes.as_slice()),
        SmartSource::Linked { path } => photocraft_io::linked::embedded_payload(&doc.metadata, path)
            .ok_or_else(|| other("photo contents editing requires embedded contents, not a linked file"))?,
    };
    if bytes.is_empty() {
        return Err(other("the smart object's contents are unavailable"));
    }
    if is_vector(bytes) || sm.vector_preview.is_some() {
        return Err(other("the smart object holds vector contents: use layer.smartObjects.exportSvg"));
    }
    Ok((name, bytes))
}

/// Both commands: an active document with no background job, editing the layer composite.
fn enabled(s: &Session) -> std::result::Result<(), String> {
    let state = s.active().ok_or("no document open")?;
    if s.job_on(state.doc.id).is_some() {
        return Err("the document has a running background job".into());
    }
    if s.tools.mask_targeted
        || state.doc.quick_mask.is_some()
        || state.channel_view.layer_mask.is_some()
        || state.channel_view.target != crate::channel_cmds::ChannelTarget::Composite
    {
        return Err("photo contents editing requires the layer composite target".into());
    }
    Ok(())
}

/// Every layer sharing `id`'s contents (they change together). Refused while those contents are
/// open in the internal editor, or when an instance is locked or not embedded raster contents.
fn check_instances(s: &Session, id: LayerId) -> Result<Vec<LayerId>> {
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let contents = smart(doc, id)?.contents_id;
    if s.smart_links.iter().any(|link| link.parent == doc.id && link.contents_id == contents) {
        return Err(other("these smart-object contents are already open in Al Majlis Design"));
    }
    let ids: Vec<_> =
        doc.walk().into_iter().filter_map(|(_, _, l)| matches!(&l.content, LayerContent::Smart(sm) if sm.contents_id == contents).then_some(l.id)).collect();
    for id in &ids {
        let locks = doc.effective_locks(*id);
        if locks.all || locks.pixels {
            return Err(other("a shared smart-object instance is locked"));
        }
        embedded(doc, smart(doc, *id)?)?;
    }
    Ok(ids)
}

/// A PNG's IHDR size, or `None` when `bytes` isn't a PNG.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(PNG_MAGIC) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some((be(16), be(20)))
}

/// The contents as a PNG at their own pixel size: PNG contents unchanged, anything else (JPEG,
/// WebP, PSD, an embedded `.pcraft`…) decoded and its composite encoded as PNG.
fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer(s, p, EXPORT, false)?;
    check_instances(s, id)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let sm = smart(doc, id)?;
    let (name, bytes) = embedded(doc, sm)?;
    let (png, (width, height)) = match png_size(bytes) {
        Some(size) => (bytes.to_vec(), size),
        None => {
            let contents = crate::smart_cmds::decode_source(&name, bytes)?;
            let r = photocraft_io::export(&contents, "contents.png", &Default::default()).map_err(|e| other(format!("can't export the contents as PNG: {e}")))?;
            (r.bytes, (contents.size.width, contents.size.height))
        }
    };
    if png.len() > MAX_PNG_BYTES {
        return Err(other("the contents exceed 64 MiB as PNG"));
    }
    Ok(json!({
        "document": doc.id.0,
        "layer": id.0,
        "fingerprint": blake3::hash(bytes).to_hex().to_string(),
        "contentsId": sm.contents_id.0,
        "fileName": name,
        "width": width,
        "height": height,
        "pngBase64": base64::engine::general_purpose::STANDARD.encode(&png),
    }))
}

/// Map the new contents' frame onto the old one, so every instance keeps its placed bounds when
/// the edited PNG has another pixel size. Warped or perspective instances must keep the size.
fn refit(sm: &mut SmartObject, old: Rect, new: Rect) -> Result<()> {
    if old == new {
        return Ok(());
    }
    if sm.warp.is_some() || sm.perspective.is_some() {
        return Err(bad(
            UPDATE,
            format!("this smart object is warped or in perspective: the edited PNG must keep its {}×{} px size", old.x1 - old.x0, old.y1 - old.y0),
        ));
    }
    let (ow, oh, nw, nh) = (f64::from(old.x1 - old.x0), f64::from(old.y1 - old.y0), f64::from(new.x1 - new.x0), f64::from(new.y1 - new.y0));
    if !(ow > 0.0 && oh > 0.0 && nw > 0.0 && nh > 0.0) {
        return Err(bad(UPDATE, "empty contents"));
    }
    let (sx, sy) = (ow / nw, oh / nh);
    let fit = Affine { m: [sx, 0.0, 0.0, sy, f64::from(old.x0) - f64::from(new.x0) * sx, f64::from(old.y0) - f64::from(new.y0) * sy] };
    sm.transform = sm.transform.mul(&fit);
    Ok(())
}

/// Swap in the edited PNG for every instance sharing the contents, as one undoable step, after
/// checking the identity `exportPng` handed out.
fn update(s: &mut Session, p: &Value) -> Result<Value> {
    let id = layer(s, p, UPDATE, true)?;
    let document = p.get("document").and_then(Value::as_u64).ok_or_else(|| bad(UPDATE, "`document` must be an integer document id"))?;
    if s.active().map(|state| state.doc.id.0) != Some(document) {
        return Err(bad(UPDATE, "the smart object's parent document is no longer active"));
    }
    let expected = p
        .get("fingerprint")
        .and_then(Value::as_str)
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| bad(UPDATE, "`fingerprint` must be a BLAKE3 hex digest"))?;
    let contents = p.get("contentsId").and_then(Value::as_u64).map(SmartContentsId).ok_or_else(|| bad(UPDATE, "`contentsId` must be an integer"))?;
    let encoded = p.get("pngBase64").and_then(Value::as_str).ok_or_else(|| bad(UPDATE, "`pngBase64` must be base64 PNG bytes"))?;
    if encoded.len() > MAX_PNG_BYTES.div_ceil(3) * 4 {
        return Err(bad(UPDATE, "the PNG exceeds 64 MiB"));
    }
    let ids = check_instances(s, id)?;
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    let target = smart(doc, id)?;
    let (name, original) = embedded(doc, target)?;
    if target.contents_id != contents || !blake3::hash(original).to_hex().as_str().eq_ignore_ascii_case(expected) {
        return Err(bad(UPDATE, "smart-object contents changed since external editing started"));
    }
    for instance in &ids {
        let (_, bytes) = embedded(doc, smart(doc, *instance)?)?;
        let matches = (bytes.len() == original.len() && std::ptr::eq(bytes.as_ptr(), original.as_ptr()))
            || blake3::hash(bytes).to_hex().as_str().eq_ignore_ascii_case(expected);
        if !matches {
            return Err(bad(UPDATE, "shared smart-object instances have conflicting contents"));
        }
    }
    let png = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| bad(UPDATE, "invalid PNG encoding"))?;
    let (w, h) = png_size(&png).ok_or_else(|| bad(UPDATE, "the edited contents must be a PNG"))?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(bad(UPDATE, "the PNG must be between 1 pixel and 24 million pixels"));
    }
    if png.as_slice() == original {
        return Ok(json!({"layer":id.0,"contentsId":contents.0,"fingerprint":expected,"updated":false}));
    }
    // Stored smart filters replay on the new contents: authorize them as updateSvg does.
    if let Some(authorize) = s.authorize {
        for instance in &ids {
            let sm = smart(doc, *instance)?;
            if sm.filters_enabled {
                for filter in sm.smart_filters.iter().filter(|filter| filter.visible) {
                    authorize(&filter.command, &filter.params)?;
                }
            }
        }
    }
    let fmt = doc.pixel_format();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
    let file_name = format!("{}.png", if stem.is_empty() { "Photo" } else { stem });
    // Decoding validates the PNG before anything changes (and warms the render's source cache).
    let new_bounds = crate::smart_cmds::source_image(&file_name, &png, fmt).map_err(|e| bad(UPDATE, format!("can't read the edited PNG: {e}")))?.bounds;
    let old_bounds = crate::smart_cmds::source_image(&name, original, fmt)?.bounds;
    let source = SmartSource::Embedded { file_name, bytes: Arc::new(png.clone()) };
    let mut replacements: Vec<Layer> = Vec::with_capacity(ids.len());
    for instance in &ids {
        let mut replacement = doc.layer(*instance).cloned().ok_or(EngineError::NoLayer(*instance))?;
        let LayerContent::Smart(sm) = &mut replacement.content else { return Err(EngineError::NoLayer(*instance)) };
        refit(sm, old_bounds, new_bounds)?;
        sm.source = source.clone();
        sm.vector_preview = None;
        replacements.push(replacement);
    }
    // Finish every fallible render before committing any document state.
    for replacement in &mut replacements {
        if !crate::smart_cmds::refresh_layer(doc, replacement)? {
            return Err(bad(UPDATE, "the edited contents could not be rendered"));
        }
        crate::smart_cmds::detach_psd(replacement);
    }
    s.edit("Update Photo Contents", |doc, _| {
        for replacement in replacements {
            let id = replacement.id;
            *doc.layer_mut(id).ok_or(EngineError::NoLayer(id))? = replacement;
        }
        Ok(())
    })?;
    Ok(json!({
        "layer": id.0,
        "contentsId": contents.0,
        "fingerprint": blake3::hash(&png).to_hex().to_string(),
        "updated": true,
        "instances": ids.iter().map(|l| l.0).collect::<Vec<_>>(),
        "width": w,
        "height": h,
    }))
}

/// Menu-less commands for the web door (`window.photocraft.exportSmartObjectBytes` /
/// `replaceSmartObjectBytes`); not journaled, so photos don't pile up in the replay journal.
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: EXPORT,
            label: "Export Photo Contents",
            menu: &[],
            shortcut: None,
            params: r#"{"layer":id?} → {document,layer,pngBase64,width,height,fileName,fingerprint,contentsId}"#,
            enabled,
            run: export,
            journal: false,
        },
        CommandSpec {
            id: UPDATE,
            label: "Update Photo Contents",
            menu: &[],
            shortcut: None,
            params: r#"{"document":id,"layer":id,"pngBase64":"PNG bytes","fingerprint":"original BLAKE3 hex","contentsId":id} → {layer,contentsId,fingerprint,updated,instances?}"#,
            enabled,
            run: update,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
