//! 巨大なツール出力では、使わない画像を Value の文字列として確保しない。
//! JSON の構文・Unicode・数値・深さは通常の Value と同じデシリアライザで検証する。

use std::fmt;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

pub(super) fn parse(line: &[u8]) -> serde_json::Result<Value> {
    if line.len() < 64 * 1024 || memchr::memmem::find(line, b"\"image_url\"").is_none() {
        return super::json_row(line);
    }
    match simdutf8::basic::from_utf8(line) {
        Ok(text) => projected(serde_json::Deserializer::from_str(text)),
        Err(_) => projected(serde_json::Deserializer::from_slice(line)),
    }
}

fn projected<'de, R: serde_json::de::Read<'de>>(
    mut de: serde_json::Deserializer<R>,
) -> serde_json::Result<Value> {
    let value = Project::Root.deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

#[derive(Clone, Copy)]
enum Project {
    Root,
    Payload,
    Output,
    Part,
    Discard,
}

impl Project {
    fn discarded(self) -> bool { matches!(self, Self::Part | Self::Discard) }
}

impl<'de> DeserializeSeed<'de> for Project {
    type Value = Value;

    fn deserialize<D>(self, de: D) -> Result<Value, D::Error>
    where D: serde::Deserializer<'de> {
        de.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Project {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(if self.discarded() { Value::Null } else { Value::Bool(value) })
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(if self.discarded() { Value::Null } else { Value::Number(value.into()) })
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(if self.discarded() { Value::Null } else { Value::Number(value.into()) })
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E> {
        Ok(if self.discarded() { Value::Null } else { Number::from_f64(value).map_or(Value::Null, Value::Number) })
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(if self.discarded() { Value::Null } else { Value::String(value.to_owned()) })
    }

    fn visit_unit<E>(self) -> Result<Value, E> { Ok(Value::Null) }

    fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
    where A: SeqAccess<'de> {
        if self.discarded() {
            while seq.next_element_seed(Project::Discard)?.is_some() {}
            return Ok(Value::Null);
        }
        let mut values = Vec::new();
        if matches!(self, Self::Output) {
            while let Some(value) = seq.next_element_seed(Project::Part)? {
                values.push(value);
            }
        } else {
            while let Some(value) = seq.next_element()? {
                values.push(value);
            }
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where A: MapAccess<'de> {
        if matches!(self, Self::Discard) {
            // IgnoredAny は不正なサロゲートも読み飛ばすため、文字列として検証する。
            while map.next_key_seed(Project::Discard)?.is_some() {
                map.next_value_seed(Project::Discard)?;
            }
            return Ok(Value::Null);
        }
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = match (self, key.as_str()) {
                (Self::Root, "payload") => map.next_value_seed(Self::Payload)?,
                (Self::Payload, "output") => map.next_value_seed(Self::Output)?,
                (Self::Part, "text") => map.next_value()?,
                (Self::Part, _) => {
                    map.next_value_seed(Self::Discard)?;
                    continue;
                }
                _ => map.next_value()?,
            };
            // 重複キーは Value と同じく、最後の値を使う。
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded(output: &str) -> Vec<u8> {
        format!(r#"{{"padding":"{}","payload":{{"output":{output}}}}}"#, " ".repeat(64 * 1024)).into_bytes()
    }

    #[test]
    fn keeps_text_parts_duplicate_keys_and_non_array_outputs() {
        for output in [
            r#"[{"image_url":"unused","text":"a"},{"text":"b","text":null},{"te\u0078t":"日本語"},"ignored",["ignored"],{"text":"c","text":"d"}]"#,
            r#"{"image_url":"retained","content":[{"text":"a"}]}"#,
            r#""image_url""#,
        ] {
            let input = padded(output);
            let original: Value = serde_json::from_slice(&input).unwrap();
            let projected = parse(&input).unwrap();
            assert_eq!(super::super::codex_output_text(original.pointer("/payload/output")),
                super::super::codex_output_text(projected.pointer("/payload/output")));
            if original["payload"]["output"].is_array() {
                assert!(projected["payload"]["output"][0].get("image_url").is_none());
            } else {
                assert_eq!(original, projected);
            }
        }
    }

    #[test]
    fn discarded_values_keep_value_validation() {
        let nested = format!("{}0{}", "[".repeat(130), "]".repeat(130));
        let invalid_utf8 = String::from("\"placeholder\"");
        for discarded in [r#""\uD800""#, r#""\uDC00""#, "1e400", "[true,]", &nested,
            r#"{"\uD800":0}"#, &invalid_utf8] {
            let output = format!(r#"[{{"image_url":{discarded},"text":"ok"}}]"#);
            let mut input = padded(&output);
            if discarded == invalid_utf8 {
                let i = memchr::memmem::find(&input, b"placeholder").unwrap();
                input[i] = 0xff;
            }
            assert!(serde_json::from_slice::<Value>(&input).is_err());
            assert!(parse(&input).is_err());
        }
        // エスケープ、数値に見えるキー、深い有効な値も捨てる前に検証する。
        let output = r#"[{"image_url":{"0":{"a":["\uD83D\uDE00",1.5e3,true,null]}}}, {"text":"kept"}]"#;
        let input = padded(output);
        assert!(serde_json::from_slice::<Value>(&input).is_ok());
        assert_eq!(super::super::codex_output_text(parse(&input).unwrap().pointer("/payload/output")), "kept");
        let mut trailing = input;
        trailing.extend_from_slice(b" garbage");
        assert!(parse(&trailing).is_err());
    }

    #[test]
    fn projected_rows_keep_utf8_values_and_error_locations() {
        let output = format!(r#"[{{"image_url":"{}","text":"日本語😀\n\uD83D\uDE00"}}]"#, "画像😀".repeat(300));
        let base = padded(&output);
        for offset in 0..64 {
            let mut input = vec![b' '; offset];
            input.extend_from_slice(&base);
            let old = projected(serde_json::Deserializer::from_slice(&input)).unwrap();
            let new = parse(&input).unwrap();
            assert_eq!(serde_json::to_vec(&old).unwrap(), serde_json::to_vec(&new).unwrap());
            let at = memchr::memmem::find(&input, "画像".as_bytes()).unwrap();
            input[at] = 0xff;
            let old = projected(serde_json::Deserializer::from_slice(&input)).unwrap_err();
            let new = parse(&input).unwrap_err();
            assert_eq!(old.classify(), new.classify());
            assert_eq!((old.line(), old.column()), (new.line(), new.column()));
            assert_eq!(old.to_string(), new.to_string());
        }
        for value in [r#""\uD800""#, r#"{"\uDC00":0}"#, "1e400", "[true,]"] {
            let input = padded(&format!(r#"[{{"image_url":{value},"text":"ok"}}]"#));
            let old = projected(serde_json::Deserializer::from_slice(&input)).unwrap_err();
            let new = parse(&input).unwrap_err();
            assert_eq!(old.to_string(), new.to_string());
        }
    }
}
