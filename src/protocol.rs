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
        .context("Google Photos did not return a signed-in page. Run 'gphotos auth chrome' again; Google may require browser verification")?;
    let value: Value = serde_json::from_str(text).context("Google Photos page format changed")?;
    for key in ["SNlM0e", "FdrFJe", "cfb2h", "Im6cmf"] {
        if !value[key].is_string() {
            bail!("Google Photos session is unavailable; sign in with Chrome and reimport");
        }
    }
    Ok(value)
}
pub fn rpc_data(body: &str, rpc: &str) -> Result<Value> {
    fn find(value: &Value, rpc: &str) -> Option<Result<Value>> {
        let array = value.as_array()?;
        if array.first().and_then(Value::as_str) == Some("wrb.fr")
            && array.get(1).and_then(Value::as_str) == Some(rpc)
        {
            return Some(match array.get(2).and_then(Value::as_str) {
                Some(text) => serde_json::from_str(text).context("Google Photos returned malformed RPC data"),
                None => Err(anyhow!("Google Photos rejected the request or the session expired; reimport with 'gphotos auth chrome'")),
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
        assert!(err.to_string().contains("session expired"));
        assert!(parse_page(&json!({"bad":true})).is_err());
        assert!(parse_page(&json!([null])).unwrap().0.is_empty());
    }
}
