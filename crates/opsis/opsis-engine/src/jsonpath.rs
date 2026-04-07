//! Minimal dot-path JSON extraction.
//!
//! Supports: `$.field`, `$.nested.field`, `$.array[*]`, `$.array[0].field`.
//! No external dependencies — just `serde_json::Value`.

use serde_json::Value;

/// Extract a single value from a JSON document using a dot-path.
///
/// Returns `None` if the path doesn't resolve.
pub fn extract(root: &Value, path: &str) -> Option<Value> {
    let path = path.strip_prefix("$.").unwrap_or(path);
    let mut current = root.clone();

    for segment in split_path(path) {
        match segment {
            PathSegment::Field(key) => {
                current = current.get(key)?.clone();
            }
            PathSegment::Index(idx) => {
                current = current.get(idx)?.clone();
            }
            PathSegment::Wildcard => {
                // For single extraction, return the array itself.
                if !current.is_array() {
                    return None;
                }
                return Some(current);
            }
        }
    }

    Some(current)
}

/// Extract a string from a JSON value at a dot-path.
pub fn extract_str(root: &Value, path: &str) -> Option<String> {
    let val = extract(root, path)?;
    match val {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Extract a float from a JSON value at a dot-path.
pub fn extract_f64(root: &Value, path: &str) -> Option<f64> {
    extract(root, path)?.as_f64()
}

/// Extract an array of values from a JSON document.
///
/// The path should contain `[*]` to iterate array elements.
/// Returns empty vec if path doesn't resolve or isn't an array.
pub fn extract_array(root: &Value, path: &str) -> Vec<Value> {
    let path = path.strip_prefix("$.").unwrap_or(path);

    // Find the [*] segment and split around it.
    let segments: Vec<&str> = path.splitn(2, "[*]").collect();

    let array_value = if segments[0].is_empty() {
        root.clone()
    } else {
        let prefix = segments[0].trim_end_matches('.');
        match extract(root, &format!("$.{prefix}")) {
            Some(v) => v,
            None => return Vec::new(),
        }
    };

    let items = match array_value.as_array() {
        Some(arr) => arr.clone(),
        None => return Vec::new(),
    };

    // If there's a suffix after [*], apply it to each element.
    if segments.len() > 1 && !segments[1].is_empty() {
        let suffix = segments[1].trim_start_matches('.');
        items
            .into_iter()
            .filter_map(|item| extract(&item, &format!("$.{suffix}")))
            .collect()
    } else {
        items
    }
}

// ── Path parsing ────────────────────────────────────────────────────

enum PathSegment<'a> {
    Field(&'a str),
    Index(usize),
    Wildcard,
}

fn split_path(path: &str) -> Vec<PathSegment<'_>> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            continue;
        }
        if let Some(bracket_pos) = part.find('[') {
            let field = &part[..bracket_pos];
            if !field.is_empty() {
                segments.push(PathSegment::Field(field));
            }
            let idx_str = &part[bracket_pos + 1..part.len() - 1]; // strip [ and ]
            if idx_str == "*" {
                segments.push(PathSegment::Wildcard);
            } else if let Ok(idx) = idx_str.parse::<usize>() {
                segments.push(PathSegment::Index(idx));
            }
        } else {
            segments.push(PathSegment::Field(part));
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_simple_field() {
        let data = json!({"name": "test", "value": 42});
        assert_eq!(extract_str(&data, "$.name"), Some("test".into()));
        assert_eq!(extract_f64(&data, "$.value"), Some(42.0));
    }

    #[test]
    fn extract_nested() {
        let data = json!({"a": {"b": {"c": "deep"}}});
        assert_eq!(extract_str(&data, "$.a.b.c"), Some("deep".into()));
    }

    #[test]
    fn extract_missing_returns_none() {
        let data = json!({"a": 1});
        assert!(extract(&data, "$.missing").is_none());
        assert!(extract(&data, "$.a.b.c").is_none());
    }

    #[test]
    fn extract_array_wildcard() {
        let data = json!({"features": [{"id": 1}, {"id": 2}, {"id": 3}]});
        let items = extract_array(&data, "$.features[*]");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["id"], 1);
    }

    #[test]
    fn extract_array_with_suffix() {
        let data = json!({"features": [
            {"properties": {"mag": 5.2}},
            {"properties": {"mag": 3.1}},
        ]});
        let mags = extract_array(&data, "$.features[*].properties.mag");
        assert_eq!(mags.len(), 2);
        assert_eq!(mags[0].as_f64(), Some(5.2));
        assert_eq!(mags[1].as_f64(), Some(3.1));
    }

    #[test]
    fn extract_indexed() {
        let data = json!({"items": ["a", "b", "c"]});
        assert_eq!(extract_str(&data, "$.items[0]"), Some("a".into()));
        assert_eq!(extract_str(&data, "$.items[2]"), Some("c".into()));
    }

    #[test]
    fn extract_f64_from_nested() {
        let data = json!({"current": {"temperature_2m": 18.5}});
        assert_eq!(extract_f64(&data, "$.current.temperature_2m"), Some(18.5));
    }

    #[test]
    fn extract_number_as_string() {
        let data = json!({"mag": 6.2});
        assert_eq!(extract_str(&data, "$.mag"), Some("6.2".into()));
    }

    #[test]
    fn geojson_usgs_like() {
        let data = json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "properties": {"mag": 6.2, "place": "10km NE of Tokyo"},
                    "geometry": {"coordinates": [139.69, 35.68, 10.0]}
                },
                {
                    "properties": {"mag": 2.1, "place": "5km S of LA"},
                    "geometry": {"coordinates": [-118.24, 34.05, 5.0]}
                }
            ]
        });
        let features = extract_array(&data, "$.features[*]");
        assert_eq!(features.len(), 2);

        let mag = extract_f64(&features[0], "$.properties.mag");
        assert_eq!(mag, Some(6.2));

        let place = extract_str(&features[0], "$.properties.place");
        assert_eq!(place, Some("10km NE of Tokyo".into()));
    }

    #[test]
    fn empty_array_returns_empty_vec() {
        let data = json!({"features": []});
        assert!(extract_array(&data, "$.features[*]").is_empty());
    }

    #[test]
    fn non_array_returns_empty_vec() {
        let data = json!({"features": "not an array"});
        assert!(extract_array(&data, "$.features[*]").is_empty());
    }
}
