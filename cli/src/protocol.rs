//! Google Photos web RPC parsing. Field locations follow xob0t/gpwc (MIT).
//! No captured account responses or credentials belong in this module or its tests.
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, FixedOffset, Utc};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize, Clone)]
pub struct Photo {
    pub id: String,
    pub filename: Option<String>,
    pub taken_at: Option<String>,
    pub media_type: String,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub url: String,
}

// Some protobuf JSON records fold sparse trailing fields into a final object.
pub fn field(value: &Value, index: usize) -> &Value {
    let Some(array) = value.as_array() else {
        return &Value::Null;
    };
    match array.get(index) {
        Some(v) if !(index == array.len() - 1 && v.is_object()) => v,
        _ => array
            .last()
            .and_then(|v| v.get((index + 1).to_string()))
            .unwrap_or(&Value::Null),
    }
}
pub fn extension<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .as_array()
        .and_then(|a| a.last())
        .and_then(|v| v.get(name))
        .unwrap_or(&Value::Null)
}
pub fn globals(html: &str) -> Result<Value> {
    let re = Regex::new(r"(?s)window\.WIZ_global_data\s*=\s*(\{.*?\})\s*;")?;
    let text = re.captures(html).and_then(|c| c.get(1)).map(|m| m.as_str())
        .context("Google Photos did not return a signed-in page. Import fresh browser cookies or an exported session; Google may require browser verification")?;
    let value: Value = serde_json::from_str(text).context("Google Photos page format changed")?;
    for key in ["SNlM0e", "FdrFJe", "cfb2h", "Im6cmf"] {
        if !value[key].is_string() {
            bail!("Google Photos session is unavailable; sign in with Chrome and reimport");
        }
    }
    Ok(value)
}
#[derive(Debug)]
pub(crate) struct RateLimit {
    pub(crate) source: &'static str,
    pub(crate) retry_after: Option<std::time::Duration>,
}

impl std::fmt::Display for RateLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Google Photos is limiting requests ({}).", self.source)?;
        match self.retry_after {
            Some(delay) => write!(f, " Retry after {} seconds.", delay.as_secs()),
            None => write!(
                f,
                " Wait before retrying. Google did not provide a reset time."
            ),
        }
    }
}

impl std::error::Error for RateLimit {}

#[derive(Debug)]
pub struct SessionExpired;

impl std::fmt::Display for SessionExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Google Photos authentication expired or is invalid. Sign in with Chrome and reimport your session")
    }
}

impl std::error::Error for SessionExpired {}

fn rpc_rejection(frame: &[Value]) -> anyhow::Error {
    let status = frame
        .get(5)
        .and_then(|value| value.get(0))
        .and_then(Value::as_i64);
    match status {
        Some(8) => RateLimit {
            source: "RPC code 8: RESOURCE_EXHAUSTED",
            retry_after: None,
        }
        .into(),
        Some(16) => SessionExpired.into(),
        Some(code) => anyhow!("Google Photos rejected the request (RPC code {code})"),
        None => anyhow!("Google Photos rejected the request without a status code"),
    }
}

pub fn rpc_data(body: &str, rpc: &str) -> Result<Value> {
    fn find(value: &Value, rpc: &str) -> Option<Result<Value>> {
        let array = value.as_array()?;
        if array.first().and_then(Value::as_str) == Some("wrb.fr")
            && array.get(1).and_then(Value::as_str) == Some(rpc)
        {
            return Some(match array.get(2).and_then(Value::as_str) {
                Some(text) => {
                    serde_json::from_str(text).context("Google Photos returned malformed RPC data")
                }
                None => Err(rpc_rejection(array)),
            });
        }
        array.iter().find_map(|v| find(v, rpc))
    }
    for line in body.lines().filter(|l| l.contains("wrb.fr")) {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if let Some(data) = find(&value, rpc) {
                return data;
            }
        }
    }
    bail!("Google Photos returned no matching RPC response; the session may have expired or the web API changed")
}
pub fn parse_page(data: &Value) -> Result<(Vec<Photo>, Option<String>)> {
    if !data.is_array() {
        bail!("Unexpected Google Photos search response");
    }
    let rows = field(data, 0);
    if !rows.is_null() && !rows.is_array() {
        bail!("Unexpected Google Photos result list");
    }
    let mut photos = Vec::new();
    for row in rows.as_array().into_iter().flatten() {
        let id = field(row, 0)
            .as_str()
            .context("Search result has no photo ID")?
            .to_owned();
        let video = !extension(row, "76647426").is_null();
        let taken_at = field(row, 2)
            .as_i64()
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .map(|dt| {
                let offset = field(row, 4).as_i64().unwrap_or(0) / 1000;
                let offset = i32::try_from(offset)
                    .ok()
                    .and_then(FixedOffset::east_opt)
                    .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
                dt.with_timezone(&offset).to_rfc3339()
            });
        photos.push(Photo {
            url: format!("https://photos.google.com/photo/{id}"),
            id,
            filename: None,
            taken_at,
            media_type: if video { "video" } else { "photo" }.into(),
            width: field(field(row, 1), 1).as_u64(),
            height: field(field(row, 1), 2).as_u64(),
        });
    }
    let next = field(data, 1)
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok((photos, next))
}
pub fn batch_payload(ids: &[String]) -> Value {
    let keys: Vec<Value> = ids.iter().map(|id| json!([id])).collect();
    let mut options = vec![Value::Null; 24];
    options.push(json!([]));
    options.extend(vec![Value::Null; 10]);
    options.push(json!([]));
    json!([[[keys], [options]]])
}
pub fn filenames(data: &Value, items: &mut [Photo]) {
    if let Some(rows) = field(field(data, 0), 1).as_array() {
        for row in rows {
            if let Some(photo) = items
                .iter_mut()
                .find(|p| Some(p.id.as_str()) == field(row, 0).as_str())
            {
                photo.filename = field(field(row, 1), 3).as_str().map(str::to_owned);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_rpc_and_sparse_photo() {
        let row = json!(["photo-key-123", ["https://example.test/thumbnail", 4032, 3024], 1700000000000i64, null, -18000000, {"76647426":[1234]}]);
        let page = json!([[row], "next"]);
        let body = format!(
            ")]}}'\n123\n{}\n",
            json!([["wrb.fr", "EzkLib", page.to_string(), null, null, null, "id"]])
        );
        let (items, next) = parse_page(&rpc_data(&body, "EzkLib").unwrap()).unwrap();
        assert_eq!(items[0].width, Some(4032));
        assert_eq!(items[0].media_type, "video");
        assert!(items[0].taken_at.as_ref().unwrap().ends_with("-05:00"));
        assert_eq!(next.as_deref(), Some("next"));
        assert_eq!(field(&json!(["a", {"13":"original"}]), 12), "original");
    }
    #[test]
    fn batch_options_are_nested_and_filenames_are_parsed() {
        let payload = batch_payload(&["photo-key-123".into()]);
        assert_eq!(payload[0][0][0][0][0], "photo-key-123");
        assert_eq!(payload[0][1][0].as_array().unwrap().len(), 36);
        let mut items = parse_page(&json!([
            [["photo-key-123", ["thumbnail", 4032, 3024], 1700000000000i64]],
            null
        ]))
        .unwrap()
        .0;
        filenames(
            &json!([[
                null,
                [["photo-key-123", [null, null, "caption", "original.jpg"]]]
            ]]),
            &mut items,
        );
        assert_eq!(items[0].filename.as_deref(), Some("original.jpg"));
    }

    #[test]
    fn rejects_login_and_error_responses_without_echoing_body() {
        assert!(globals("<html>login secret</html>")
            .unwrap_err()
            .to_string()
            .contains("signed-in"));
        let err = rpc_data(r#"[["wrb.fr","EzkLib",null]]"#, "EzkLib").unwrap_err();
        assert!(err.to_string().contains("without a status code"));
        let limited = rpc_data(
            r#"[["wrb.fr","EzkLib",null,null,null,[8,"private details"]]]"#,
            "EzkLib",
        )
        .unwrap_err()
        .to_string();
        assert!(limited.contains("limiting requests") && limited.contains("RESOURCE_EXHAUSTED"));
        assert!(!limited.contains("private details"));
        let unauthenticated =
            rpc_data(r#"[["wrb.fr","EzkLib",null,null,null,[16]]]"#, "EzkLib").unwrap_err();
        assert!(unauthenticated
            .to_string()
            .contains("reimport your session"));
        assert!(parse_page(&json!({"bad":true})).is_err());
        assert!(parse_page(&json!([null])).unwrap().0.is_empty());
    }
}

#[derive(Serialize, Clone)]
pub struct Person {
    pub id: String,
    pub name: Option<String>,
}
#[derive(Serialize)]
pub struct Camera {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub aperture_f_number: Option<f64>,
    pub iso: Option<u64>,
    pub exposure_seconds: Option<f64>,
}
#[derive(Serialize)]
pub struct Location {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub labels: Vec<String>,
}
#[derive(Serialize)]
pub struct Album {
    pub id: String,
    pub title: Option<String>,
}
#[derive(Serialize)]
pub struct Inspection {
    pub id: String,
    pub url: String,
    pub filename: Option<String>,
    pub description: Option<String>,
    pub caption: Option<String>,
    pub taken_at: Option<String>,
    pub uploaded_at: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub size_bytes: Option<u64>,
    pub media_type: String,
    pub duration_ms: Option<u64>,
    pub is_live_photo: bool,
    pub camera: Option<Camera>,
    pub location: Option<Location>,
    pub people: Vec<Person>,
    pub people_status: String,
    pub faces: Vec<Face>,
    pub faces_status: String,
    pub albums: Vec<Album>,
    pub source: Vec<String>,
    pub is_favorite: Option<bool>,
    pub is_archived: Option<bool>,
    pub storage_quality: Option<String>,
    pub storage_used_bytes: Option<u64>,
}
fn string(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}
fn timestamp(value: &Value, offset: &Value) -> Option<String> {
    let dt = DateTime::<Utc>::from_timestamp_millis(value.as_i64()?)?;
    let offset = offset.as_i64().unwrap_or(0) / 1000;
    let tz = i32::try_from(offset)
        .ok()
        .and_then(FixedOffset::east_opt)
        .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    Some(dt.with_timezone(&tz).to_rfc3339())
}
pub fn parse_people(rows: &Value) -> Result<Vec<Person>> {
    let rows = rows
        .as_array()
        .context("Google returned an unexpected face-group list")?;
    let mut people = Vec::new();
    for row in rows {
        let id = field(field(row, 0), 0)
            .as_str()
            .context("Google face-group entry has no group ID")?;
        people.push(Person {
            id: id.into(),
            name: string(field(field(row, 1), 5)),
        });
    }
    Ok(people)
}
pub fn people_index(data: &Value) -> Result<Vec<Person>> {
    let sections = field(data, 1)
        .as_array()
        .context("Google returned an unexpected people index")?;
    for section in sections {
        if field(section, 0).as_u64() == Some(1) {
            return parse_people(field(section, 1));
        }
    }
    bail!(
        "Google did not return face groups for this account; check face grouping in Google Photos"
    )
}
/// Structured face-group filter captured from Google's People & Pets navigation.
pub fn keyword_search_payload(query: &str, token: Option<&str>) -> Value {
    keyword_search_payload_ordered(query, token, false)
}
pub fn keyword_search_payload_ordered(query: &str, token: Option<&str>, recent: bool) -> Value {
    json!([
        null,
        null,
        token,
        null,
        if recent { 50 } else { 100 },
        null,
        if recent { 1 } else { 2 },
        [query],
        null,
        [1, 1]
    ])
}
pub fn person_search_payload(id: &str, token: Option<&str>) -> Value {
    let face = json!([null, id]);
    let filter = json!([null, [[[[face]]]]]);
    json!([null, null, token, null, 50, null, 1, filter])
}
/// None means Google supplied no face metadata; it is never treated as a match.
pub fn photo_has_person(ext: &Value, photo_id: &str, person_id: &str) -> Result<Option<bool>> {
    photo_has_people(ext, photo_id, &[person_id.to_owned()])
}
pub fn photo_has_people(ext: &Value, photo_id: &str, required: &[String]) -> Result<Option<bool>> {
    let item = field(ext, 0);
    if item.is_null() {
        return Ok(None);
    }
    if field(item, 0).as_str() != Some(photo_id) {
        bail!("Google returned metadata for an unexpected photo");
    }
    let tags = field(item, 18);
    if !tags.is_array() {
        return Ok(None);
    }
    let people = parse_people(tags)?;
    Ok(Some(
        required.iter().all(|id| people.iter().any(|p| p.id == *id)),
    ))
}
/// Decode read-only batched RPCs by echoed request ID, never response arrival order.
pub fn rpc_batch_data(body: &str, rpc: &str, count: usize) -> Result<Vec<Value>> {
    fn collect(
        value: &Value,
        rpc: &str,
        out: &mut std::collections::HashMap<String, Value>,
    ) -> Result<()> {
        let Some(a) = value.as_array() else {
            return Ok(());
        };
        if a.first().and_then(Value::as_str) == Some("wrb.fr")
            && a.get(1).and_then(Value::as_str) == Some(rpc)
        {
            let text = a
                .get(2)
                .and_then(Value::as_str)
                .ok_or_else(|| rpc_rejection(a))?;
            let id = a
                .get(6)
                .and_then(Value::as_str)
                .context("Batched RPC has no request ID")?;
            if out
                .insert(
                    id.into(),
                    serde_json::from_str(text).context("Malformed batched metadata")?,
                )
                .is_some()
            {
                bail!("Duplicate batched metadata response ID");
            }
        } else {
            for child in a {
                collect(child, rpc, out)?;
            }
        }
        Ok(())
    }
    let mut values = std::collections::HashMap::new();
    for line in body.lines().filter(|l| l.contains("wrb.fr")) {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            collect(&v, rpc, &mut values)?;
        }
    }
    (0..count)
        .map(|i| {
            values
                .remove(&format!("p{i}"))
                .context("Google omitted a batched metadata response")
        })
        .collect()
}
pub fn parse_inspection(ext: &Value, info: &Value, id: &str, account: u32) -> Result<Inspection> {
    let item = field(ext, 0);
    if field(item, 0).as_str() != Some(id) {
        bail!("Google Photos returned no metadata for this photo ID");
    }
    let base = field(info, 0);
    let raw_camera = field(item, 23);
    let raw_camera = if raw_camera.is_array() {
        raw_camera
    } else {
        field(field(field(base, 1), 8), 4)
    };
    let camera = raw_camera.as_array().map(|_| Camera {
        make: string(field(raw_camera, 0)),
        model: string(field(raw_camera, 1)),
        lens: string(field(raw_camera, 2)),
        focal_length_mm: field(raw_camera, 3).as_f64(),
        aperture_f_number: field(raw_camera, 4).as_f64(),
        iso: field(raw_camera, 5).as_u64(),
        exposure_seconds: field(raw_camera, 6).as_f64(),
    });
    let gps = field(field(item, 9), 0);
    let gps = if gps.is_array() {
        gps
    } else {
        field(field(item, 13), 0)
    };
    let latitude = field(gps, 0)
        .as_f64()
        .map(|n| n / 1e7)
        .filter(|n| (-90.0..=90.0).contains(n));
    let longitude = field(gps, 1)
        .as_f64()
        .map(|n| n / 1e7)
        .filter(|n| (-180.0..=180.0).contains(n));
    let mut labels = Vec::new();
    if let Some(groups) = field(field(item, 13), 2).as_array() {
        for group in groups {
            if let Some(names) = field(group, 1).as_array() {
                for label in names {
                    if let Some(name) = string(field(label, 0)) {
                        if !labels.contains(&name) {
                            labels.push(name);
                        }
                    }
                }
            }
        }
    }
    let location = if latitude.is_some() || longitude.is_some() || !labels.is_empty() {
        Some(Location {
            latitude,
            longitude,
            labels,
        })
    } else {
        None
    };
    let people_rows = field(item, 18);
    let (people, people_status) = if people_rows.is_array() {
        (parse_people(people_rows)?, "available")
    } else {
        (Vec::new(), "not_returned")
    };
    let (faces, faces_status) = match parse_faces(item, &people)? {
        Some(faces) => (faces, "available"),
        None => (Vec::new(), "not_returned"),
    };
    let mut albums = Vec::new();
    if let Some(rows) = field(item, 19).as_array() {
        for row in rows {
            if let Some(id) = string(field(row, 0)) {
                albums.push(Album {
                    id,
                    title: string(field(extension(row, "72930366"), 1)),
                });
            }
        }
    }
    let mut source = Vec::new();
    if let Some(source_name) = match field(field(item, 27), 0).as_u64() {
        Some(1) => Some("mobile"),
        Some(2) => Some("web"),
        Some(3) => Some("shared"),
        Some(4) => Some("partner_shared"),
        Some(7) => Some("drive"),
        Some(8) => Some("computer"),
        Some(11) => Some("gmail"),
        _ => None,
    } {
        source.push(source_name.into());
    }
    if let Some(platform) = match field(field(field(item, 27), 1), 2).as_u64() {
        Some(1) => Some("android"),
        Some(3) => Some("ios"),
        _ => None,
    } {
        source.push(platform.into());
    }
    let video = extension(base, "76647426");
    let quality = match field(field(item, 30), 2).as_u64() {
        Some(2) => Some("original".into()),
        Some(1) => Some("storage_saver".into()),
        _ => None,
    };
    Ok(Inspection {
        id: id.into(),
        url: format!("https://photos.google.com/u/{account}/photo/{id}"),
        filename: string(field(item, 2)),
        description: string(field(item, 1)),
        caption: string(field(item, 1)),
        taken_at: timestamp(field(item, 3), field(item, 4)),
        uploaded_at: timestamp(field(base, 5), field(base, 4)),
        width: field(item, 6).as_u64(),
        height: field(item, 7).as_u64(),
        size_bytes: field(item, 5).as_u64(),
        media_type: if video.is_null() { "photo" } else { "video" }.into(),
        duration_ms: field(video, 0).as_u64(),
        is_live_photo: !extension(base, "146008172").is_null(),
        camera,
        location,
        people,
        people_status: people_status.into(),
        faces,
        faces_status: faces_status.into(),
        albums,
        source,
        is_favorite: field(extension(base, "163238866"), 0).as_bool(),
        is_archived: field(base, 13).as_bool(),
        storage_quality: quality,
        storage_used_bytes: field(field(item, 30), 1).as_u64(),
    })
}

/// Normalized face rectangle (0.0-1.0 of the image width/height), origin top-left.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FaceBox {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}
/// The same rectangle in pixels of the stored image, when its dimensions are known.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PixelBox {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}
#[derive(Serialize, Clone)]
pub struct Face {
    /// 0-based position after sorting faces left to right (then top to bottom).
    pub index: usize,
    /// Google's detected-face key; stable across requests for the same face.
    pub face_id: String,
    #[serde(rename = "box")]
    pub bbox: FaceBox,
    pub box_px: Option<PixelBox>,
    /// The face group this face is assigned to, if any (named or unnamed).
    pub person: Option<Person>,
    pub tagged: bool,
}
/// Every detected face in an extended-photo record (`fDcn4b` response `[0]`).
///
/// Field 21 lists detected faces as `[[face_id], [left, top, right, bottom], ...]`.
/// Field 20 links clusters to faces: `[[cluster_id, [owner, n]], [face_id, ...] | null, ...]`.
/// A face is tagged when one of its linked clusters is a face group listed in field 18.
/// Internal clusters that are not face groups are ignored. Returns None when Google
/// did not return the face list at all (unknown, not "no faces").
pub fn parse_faces(item: &Value, people: &[Person]) -> Result<Option<Vec<Face>>> {
    let Some(rows) = field(item, 21).as_array() else {
        return Ok(None);
    };
    let width = field(item, 6).as_u64().filter(|w| *w > 0);
    let height = field(item, 7).as_u64().filter(|h| *h > 0);
    let links = field(item, 20).as_array().cloned().unwrap_or_default();
    let mut faces = Vec::new();
    for row in rows {
        let face_id = field(field(row, 0), 0)
            .as_str()
            .context("Google returned a detected face without an ID")?;
        let coords = (0..4)
            .map(|i| field(field(row, 1), i).as_f64())
            .collect::<Option<Vec<_>>>()
            .context("Google returned a detected face without a bounding box")?;
        let bbox = FaceBox {
            left: coords[0],
            top: coords[1],
            right: coords[2],
            bottom: coords[3],
        };
        let box_px = match (width, height) {
            (Some(w), Some(h)) => {
                let px = |v: f64, size: u64| (v.clamp(0.0, 1.0) * size as f64).round() as u64;
                let (x, y) = (px(bbox.left, w), px(bbox.top, h));
                Some(PixelBox {
                    x,
                    y,
                    width: px(bbox.right, w).saturating_sub(x),
                    height: px(bbox.bottom, h).saturating_sub(y),
                })
            }
            _ => None,
        };
        let person = links
            .iter()
            .filter(|link| field(field(link, 1), 0).as_str() == Some(face_id))
            .filter_map(|link| field(field(link, 0), 0).as_str())
            .find_map(|cluster| people.iter().find(|p| p.id == cluster))
            .cloned();
        faces.push(Face {
            index: 0,
            face_id: face_id.into(),
            bbox,
            box_px,
            tagged: person.is_some(),
            person,
        });
    }
    faces.sort_by(|a, b| {
        a.bbox
            .left
            .total_cmp(&b.bbox.left)
            .then(a.bbox.top.total_cmp(&b.bbox.top))
            .then(a.face_id.cmp(&b.face_id))
    });
    for (i, face) in faces.iter_mut().enumerate() {
        face.index = i;
    }
    Ok(Some(faces))
}
/// What `tag` needs from one extended-photo record.
pub struct PhotoFaces {
    pub id: String,
    pub filename: Option<String>,
    /// Field 11: the media key Google's web client sends with face edits.
    pub media_key: String,
    pub faces: Vec<Face>,
}
pub fn parse_photo_faces(ext: &Value, id: &str) -> Result<PhotoFaces> {
    let item = field(ext, 0);
    if field(item, 0).as_str() != Some(id) {
        bail!("Google Photos returned no metadata for this photo ID");
    }
    let people = match field(item, 18) {
        rows if rows.is_array() => parse_people(rows)?,
        _ => Vec::new(),
    };
    let faces = parse_faces(item, &people)?.context(
        "Google returned no face list for this photo; it may be a video or have no detected faces",
    )?;
    Ok(PhotoFaces {
        id: id.into(),
        filename: string(field(item, 2)),
        media_key: string(field(item, 11))
            .context("Google returned no media key for this photo; face edits are unavailable")?,
        faces,
    })
}
/// `k9FTge` payload sent by photos.google.com when "Edit faces" assigns a detected
/// face to an existing face group and the user presses Done.
pub fn tag_face_payload(media_key: &str, face_id: &str, person_id: &str) -> Value {
    let mut options = vec![Value::Null; 20];
    options.push(json!([]));
    json!([
        [null, media_key],
        [[[face_id], [person_id]]],
        [options],
        [[], [null, null, [null, 1]]]
    ])
}
/// Confirms that Google's updated record (`k9FTge` response `[0][0]`) links the
/// face to the person. Anything else is an error, so a write is never assumed.
pub fn confirm_face_tag(
    data: &Value,
    photo_id: &str,
    face_id: &str,
    person_id: &str,
) -> Result<()> {
    let record = field(field(data, 0), 0);
    if field(record, 0).as_str() != Some(photo_id) {
        bail!(
            "Google's face-edit response did not describe this photo; check with 'gphotos inspect'"
        );
    }
    let linked = field(field(record, 1), 22).as_array().is_some_and(|links| {
        links.iter().any(|link| {
            field(field(link, 0), 0).as_str() == Some(person_id)
                && field(field(link, 1), 0).as_str() == Some(face_id)
        })
    });
    if !linked {
        bail!("Google accepted the request but did not report the face as tagged; check with 'gphotos inspect'");
    }
    Ok(())
}

#[cfg(test)]
mod inspection_tests {
    use super::*;
    fn photo(people: Value) -> Value {
        let mut item = vec![Value::Null; 34];
        item[0] = json!("synthetic-photo-id");
        item[1] = json!("A sample caption");
        item[2] = json!("photo.jpg");
        item[3] = json!(1700000000000i64);
        item[4] = json!(-18000000);
        item[5] = json!(123456);
        item[6] = json!(4000);
        item[7] = json!(3000);
        item[9] = json!([[400000000, -750000000]]);
        item[13] = json!([
            [400000000, -750000000],
            false,
            [[null, [["Sample City", null, 1]]]]
        ]);
        item[18] = people;
        item[23] = json!([
            "Example",
            "Camera 1",
            "Example 24–70mm",
            6.93,
            4.0,
            320,
            0.008333
        ]);
        json!([item])
    }
    #[test]
    fn parses_metadata_and_named_and_unnamed_tags_without_using_owner() {
        let ext = photo(json!([
            [
                ["group-alice-123"],
                ["thumbnail", null, null, null, null, "Alice"]
            ],
            [["group-unnamed-123"], ["thumbnail"]]
        ]));
        let info = json!([
            ["synthetic-photo-id"],
            null,
            null,
            [
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                null,
                ["Owner Is Not A Tag"]
            ]
        ]);
        let value = parse_inspection(&ext, &info, "synthetic-photo-id", 2).unwrap();
        assert_eq!(value.people.len(), 2);
        assert_eq!(value.people[0].name.as_deref(), Some("Alice"));
        assert!(value.people[1].name.is_none());
        assert_eq!(value.people_status, "available");
        assert_eq!(value.filename.as_deref(), Some("photo.jpg"));
        assert_eq!(value.caption.as_deref(), Some("A sample caption"));
        assert_eq!(value.caption, value.description);
        assert!(value.taken_at.as_deref().unwrap().ends_with("-05:00"));
        let camera = value.camera.unwrap();
        assert_eq!(camera.make.as_deref(), Some("Example"));
        assert_eq!(camera.model.as_deref(), Some("Camera 1"));
        assert_eq!(camera.lens.as_deref(), Some("Example 24–70mm"));
        assert_eq!(camera.iso, Some(320));
        let location = value.location.unwrap();
        assert_eq!(location.latitude, Some(40.0));
        assert_eq!(location.longitude, Some(-75.0));
        assert_eq!(location.labels, vec!["Sample City"]);
        assert!(value.url.contains("/u/2/"));
    }
    #[test]
    fn missing_tags_are_unknown_not_no_people_and_wrong_photo_is_rejected() {
        let value =
            parse_inspection(&photo(Value::Null), &json!([]), "synthetic-photo-id", 0).unwrap();
        assert_eq!(value.people_status, "not_returned");
        assert!(value.people.is_empty());
        assert!(parse_inspection(&photo(Value::Null), &json!([]), "wrong-photo-id", 0).is_err());
    }
    #[test]
    fn combined_search_matches_exact_tag_ids_and_never_infers_missing_people() {
        let tagged = photo(json!([[
            ["group-alice-123"],
            ["thumbnail", null, null, null, null, "Alice"]
        ]]));
        assert_eq!(
            photo_has_person(&tagged, "synthetic-photo-id", "group-alice-123").unwrap(),
            Some(true)
        );
        assert_eq!(
            photo_has_person(&tagged, "synthetic-photo-id", "another-group-id").unwrap(),
            Some(false)
        );
        assert_eq!(
            photo_has_person(&photo(Value::Null), "synthetic-photo-id", "group-alice-123").unwrap(),
            None
        );
        assert!(photo_has_person(&tagged, "wrong-photo-id", "group-alice-123").is_err());
    }

    #[test]
    fn face_index_and_structured_filter_preserve_group_identity() {
        let index = json!([
            "opaque",
            [[
                1,
                [[
                    ["group-alice-123"],
                    ["thumbnail", null, null, null, null, "Alice"]
                ]]
            ]]
        ]);
        assert_eq!(people_index(&index).unwrap()[0].id, "group-alice-123");
        let payload = person_search_payload("group-alice-123", Some("next"));
        assert!(payload[0].is_null());
        assert!(payload[7][0].is_null());
        assert_eq!(payload[6], 1);
        assert_eq!(payload[2], "next");
        assert_eq!(payload[7][1][0][0][0][0][1], "group-alice-123");
        let keywords = keyword_search_payload("birthday cake", Some("next-page"));
        assert!(keywords[0].is_null());
        assert_eq!(keywords[7][0], "birthday cake");
        assert_eq!(keywords[2], "next-page");
        assert_eq!(keywords[6], 2);
    }
}

#[cfg(test)]
mod multi_person_tests {
    use super::*;
    #[test]
    fn both_people_means_and_not_or_even_when_labels_are_identical() {
        let mut item = vec![Value::Null; 19];
        item[0] = json!("photo-example-id");
        item[18] = json!([[["group-a-id"], [null, null, null, null, null, "Same name"]]]);
        let required = vec!["group-a-id".into(), "group-b-id".into()];
        assert_eq!(
            photo_has_people(&json!([item.clone()]), "photo-example-id", &required).unwrap(),
            Some(false)
        );
        item[18].as_array_mut().unwrap().push(json!([
            ["group-b-id"],
            [null, null, null, null, null, "Same name"]
        ]));
        assert_eq!(
            photo_has_people(&json!([item]), "photo-example-id", &required).unwrap(),
            Some(true)
        );
    }
    #[test]
    fn batched_metadata_is_matched_by_id_when_google_reorders_responses() {
        let body = json!([
            [
                "wrb.fr",
                "fDcn4b",
                json!(["second"]).to_string(),
                null,
                null,
                null,
                "p1"
            ],
            [
                "wrb.fr",
                "fDcn4b",
                json!(["first"]).to_string(),
                null,
                null,
                null,
                "p0"
            ]
        ])
        .to_string();
        let values = rpc_batch_data(&body, "fDcn4b", 2).unwrap();
        assert_eq!(values, vec![json!(["first"]), json!(["second"])]);
        assert!(rpc_batch_data(&body, "fDcn4b", 3).is_err());
    }
}

#[cfg(test)]
mod face_tests {
    //! Fixtures are recorded web-client traffic with every ID, name, URL, and
    //! timestamp replaced by synthetic values (see tests/fixtures).
    use super::*;
    const PHOTO: &str = "AF1QipSynthetic001xxxxxxxxxxxxxxxxxxxxxxx";
    const PERSON: &str = "AF1QipSynthetic002xxxxxxxxxxxxxxxxxx";
    const TAGGED_FACE: &str = "AF1QipSynthetic017xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    const UNTAGGED_FACE: &str = "AF1QipSynthetic019xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    fn ext() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/fdcn4b_faces.json")).unwrap()
    }

    #[test]
    fn lists_tagged_and_untagged_faces_left_to_right_with_boxes() {
        let photo = parse_photo_faces(&ext(), PHOTO).unwrap();
        assert_eq!(photo.media_key, "synthetic-media-key");
        assert_eq!(photo.faces.len(), 2);
        let (left, right) = (&photo.faces[0], &photo.faces[1]);
        assert_eq!((left.index, left.face_id.as_str()), (0, UNTAGGED_FACE));
        assert!(!left.tagged && left.person.is_none());
        assert_eq!((right.index, right.face_id.as_str()), (1, TAGGED_FACE));
        let person = right.person.as_ref().unwrap();
        assert_eq!(
            (person.id.as_str(), person.name.as_deref()),
            (PERSON, Some("Alice Example"))
        );
        assert!((right.bbox.left - 0.5523203).abs() < 1e-9);
        // 1099 x 1080 image
        assert_eq!(
            right.box_px,
            Some(PixelBox {
                x: 607,
                y: 94,
                width: 247,
                height: 287
            })
        );
    }

    #[test]
    fn internal_clusters_are_not_reported_as_people_and_missing_lists_are_unknown() {
        let mut value = ext();
        // Drop the face-group list: the face is still linked to clusters, but none
        // of them is a face group, so it must read as untagged.
        value[0][18] = Value::Null;
        let photo = parse_photo_faces(&value, PHOTO).unwrap();
        assert!(photo.faces.iter().all(|f| !f.tagged));
        value[0][21] = Value::Null;
        assert!(parse_photo_faces(&value, PHOTO).is_err());
        assert!(parse_faces(&value[0], &[]).unwrap().is_none());
        assert!(parse_photo_faces(&ext(), "AF1QipSomeOtherPhoto").is_err());
    }

    #[test]
    fn inspection_includes_faces() {
        let info = json!([[PHOTO]]);
        let value = parse_inspection(&ext(), &info, PHOTO, 0).unwrap();
        assert_eq!(value.faces_status, "available");
        assert_eq!(value.faces.len(), 2);
        assert_eq!(value.people.len(), 1);
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["faces"][1]["box"]["right"], json!(0.77707005));
        assert_eq!(json["faces"][1]["person"]["name"], json!("Alice Example"));
        assert_eq!(json["faces"][0]["tagged"], json!(false));
    }

    #[test]
    fn tag_payload_matches_the_recorded_web_request() {
        let recorded: Value =
            serde_json::from_str(include_str!("../tests/fixtures/k9ftge_request.json")).unwrap();
        assert_eq!(
            tag_face_payload("synthetic-media-key", TAGGED_FACE, PERSON),
            recorded
        );
    }

    #[test]
    fn tag_response_must_link_the_face_to_the_person() {
        let body = include_str!("../tests/fixtures/k9ftge_response.txt");
        let data = rpc_data(body, "k9FTge").unwrap();
        confirm_face_tag(&data, PHOTO, TAGGED_FACE, PERSON).unwrap();
        assert!(confirm_face_tag(&data, PHOTO, UNTAGGED_FACE, PERSON).is_err());
        assert!(confirm_face_tag(&data, PHOTO, TAGGED_FACE, "AF1QipOther").is_err());
        assert!(confirm_face_tag(&data, "AF1QipOtherPhoto", TAGGED_FACE, PERSON).is_err());
        assert!(confirm_face_tag(&json!([]), PHOTO, TAGGED_FACE, PERSON).is_err());
    }
}
