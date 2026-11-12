use super::*;
use base64::Engine as _;
use photocraft_color::{Color, ColorMode, SampleType};
use photocraft_doc::Locks;
use photocraft_geom::{Point, Size};

fn png(w: u32, h: u32, rgba: [f32; 4]) -> Vec<u8> {
    let doc = Document::with_background("p", Size::new(w, h), ColorMode::Rgb, SampleType::U8, Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]));
    photocraft_io::export(&doc, "png", &Default::default()).unwrap().bytes
}

fn session_with(name: &str, bytes: Vec<u8>) -> (Session, LayerId) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":32,"height":24})).unwrap();
    let result = crate::file_cmds::place_bytes(&mut s, name, bytes, None, &json!({"smartObject":true})).unwrap();
    (s, LayerId(result["layer"].as_u64().unwrap()))
}

fn session() -> (Session, LayerId) {
    session_with("photo.png", png(8, 6, [1.0, 0.0, 0.0, 1.0]))
}

fn request(handoff: &Value, id: LayerId, png: &[u8]) -> Value {
    json!({"document":handoff["document"],"layer":id.0,"pngBase64":base64::engine::general_purpose::STANDARD.encode(png),"fingerprint":handoff["fingerprint"],"contentsId":handoff["contentsId"]})
}

fn bounds(s: &Session, id: LayerId) -> Rect {
    smart(&s.active().unwrap().doc, id).unwrap().cache.as_ref().unwrap().content_bounds()
}

fn fails_without_edit(s: &mut Session, params: Value) -> String {
    let before = s.active().unwrap().doc.clone();
    let revision = s.active().unwrap().revision;
    let error = s.execute(UPDATE, params).unwrap_err().to_string();
    assert_eq!(s.active().unwrap().doc, before);
    assert_eq!(s.active().unwrap().revision, revision);
    error
}

#[test]
fn export_hands_out_the_png_contents_and_their_identity() {
    let red = png(8, 6, [1.0, 0.0, 0.0, 1.0]);
    let (mut s, id) = session_with("photo.png", red.clone());
    let revision = s.active().unwrap().revision;
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode(handoff["pngBase64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes, red, "PNG contents leave unchanged");
    assert_eq!((handoff["width"].as_u64(), handoff["height"].as_u64()), (Some(8), Some(6)));
    assert_eq!(handoff["fingerprint"], blake3::hash(&red).to_hex().to_string());
    let doc = &s.active().unwrap().doc;
    assert_eq!(handoff["document"], doc.id.0);
    assert_eq!(handoff["contentsId"], smart(doc, id).unwrap().contents_id.0);
    assert_eq!(s.active().unwrap().revision, revision, "export is not an edit");
}

#[test]
fn non_png_contents_export_as_png_at_their_own_size() {
    let doc = Document::with_background("p", Size::new(10, 4), ColorMode::Rgb, SampleType::U8, Color::rgba(0.0, 1.0, 0.0, 1.0));
    let jpeg = photocraft_io::export(&doc, "jpg", &Default::default()).unwrap().bytes;
    let (mut s, id) = session_with("photo.jpg", jpeg.clone());
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode(handoff["pngBase64"].as_str().unwrap()).unwrap();
    assert_eq!(png_size(&bytes), Some((10, 4)));
    assert_eq!(handoff["fingerprint"], blake3::hash(&jpeg).to_hex().to_string(), "the fingerprint is of the stored contents");
}

#[test]
fn update_swaps_contents_keeps_placement_and_undoes() {
    let (mut s, id) = session();
    let before = s.active().unwrap().doc.clone();
    let placed = bounds(&s, id);
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    let blue = png(8, 6, [0.0, 0.0, 1.0, 1.0]);
    let result = s.execute(UPDATE, request(&handoff, id, &blue)).unwrap();
    assert_eq!(result["updated"], true);
    assert_eq!(result["fingerprint"], blake3::hash(&blue).to_hex().to_string());
    assert_eq!(bounds(&s, id), placed);
    let cache = smart(&s.active().unwrap().doc, id).unwrap().cache.clone().unwrap();
    let (cx, cy) = ((placed.x0 + placed.x1) / 2, (placed.y0 + placed.y1) / 2);
    assert_eq!(cache.rgba(cx, cy), [0.0, 0.0, 1.0, 1.0]);
    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc, before);
}

#[test]
fn another_pixel_size_is_fitted_to_the_same_bounds() {
    let (mut s, id) = session();
    let old = smart(&s.active().unwrap().doc, id).unwrap().transform;
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    s.execute(UPDATE, request(&handoff, id, &png(16, 12, [0.0, 0.0, 1.0, 1.0]))).unwrap();
    let new = smart(&s.active().unwrap().doc, id).unwrap().transform;
    // The new 16×12 frame lands exactly where the old 8×6 frame was.
    for ((nx, ny), (ox, oy)) in [((0.0, 0.0), (0.0, 0.0)), ((16.0, 12.0), (8.0, 6.0)), ((16.0, 0.0), (8.0, 0.0))] {
        let (a, b) = (new.apply(Point::new(nx, ny)), old.apply(Point::new(ox, oy)));
        assert!((a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9, "{a:?} vs {b:?}");
    }
}

#[test]
fn stale_or_foreign_identity_is_refused() {
    let (mut s, id) = session();
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    let green = png(8, 6, [0.0, 1.0, 0.0, 1.0]);
    let mut wrong_contents = request(&handoff, id, &green);
    wrong_contents["contentsId"] = json!(handoff["contentsId"].as_u64().unwrap() + 1);
    assert!(fails_without_edit(&mut s, wrong_contents).contains("changed"));
    let mut wrong_document = request(&handoff, id, &green);
    wrong_document["document"] = json!(handoff["document"].as_u64().unwrap() + 1);
    fails_without_edit(&mut s, wrong_document);
    s.execute(UPDATE, request(&handoff, id, &png(8, 6, [0.0, 0.0, 1.0, 1.0]))).unwrap();
    // The first handoff is stale once the contents changed.
    assert!(fails_without_edit(&mut s, request(&handoff, id, &green)).contains("changed"));
}

#[test]
fn unchanged_contents_are_no_history_step() {
    let red = png(8, 6, [1.0, 0.0, 0.0, 1.0]);
    let (mut s, id) = session_with("photo.png", red.clone());
    let revision = s.active().unwrap().revision;
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    assert_eq!(s.execute(UPDATE, request(&handoff, id, &red)).unwrap()["updated"], false);
    assert_eq!(s.active().unwrap().revision, revision);
}

#[test]
fn only_png_into_raster_smart_objects() {
    let (mut s, id) = session();
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"/>"#;
    fails_without_edit(&mut s, request(&handoff, id, svg));
    let mut garbage = request(&handoff, id, b"");
    garbage["pngBase64"] = json!("not base64!");
    fails_without_edit(&mut s, garbage);
    let mut missing = request(&handoff, id, b"");
    missing.as_object_mut().unwrap().remove("layer");
    fails_without_edit(&mut s, missing);
    // A vector smart object goes through exportSvg; a pixel layer isn't a smart object.
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6"><rect width="8" height="6" fill="red"/></svg>"#;
    let (mut v, vid) = session_with("Artwork.svg", svg.as_bytes().to_vec());
    assert!(v.execute(EXPORT, json!({"layer":vid.0})).unwrap_err().to_string().contains("exportSvg"));
    v.execute("layer.new.layer", json!({})).unwrap();
    let pixel = v.active().unwrap().active_layer.unwrap();
    assert!(v.execute(EXPORT, json!({"layer":pixel.0})).is_err());
}

#[test]
fn locked_or_internally_open_contents_are_refused() {
    let (mut s, id) = session();
    let handoff = s.execute(EXPORT, json!({"layer":id.0})).unwrap();
    s.edit("lock", |doc, _| {
        doc.layer_mut(id).unwrap().locks = Locks { all: true, ..Default::default() };
        Ok(())
    })
    .unwrap();
    assert!(fails_without_edit(&mut s, request(&handoff, id, &png(8, 6, [0.0, 0.0, 1.0, 1.0]))).contains("locked"));
    assert!(s.execute(EXPORT, json!({"layer":id.0})).is_err());
}

#[test]
fn commands_are_registered_menu_less_and_unjournaled() {
    for id in [EXPORT, UPDATE] {
        let spec = crate::commands::find(id).unwrap_or_else(|| panic!("{id}"));
        assert!(spec.menu.is_empty() && !spec.journal, "{id}");
    }
}
