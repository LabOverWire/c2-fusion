use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::CodecError;

pub fn parse(bytes: &[u8]) -> Result<Value, CodecError> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|strict| strict.0)
        .map_err(|e| CodecError::Decode(e.to_string()))
}

struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value without repeated object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom(format!("{value} is not a JSON number")))
    }

    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(Strict(item)) = items.next_element()? {
            values.push(item);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("repeated key {key}")));
            }
            let Strict(value) = entries.next_value()?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_serde_json_when_keys_are_unique() {
        let text = br#"{"a":[1,-2,3.5,"x",true,null,{"b":{}}],"c":"d"}"#;
        let expected: Value = serde_json::from_slice(text).expect("json");
        assert_eq!(parse(text).expect("strict"), expected);
    }

    #[test]
    fn refuses_repeated_keys_at_any_depth() {
        for text in [
            r#"{"a":1,"a":2}"#,
            r#"{"x":{"a":1,"a":1}}"#,
            r#"[{"a":1},{"b":[{"c":1,"c":2}]}]"#,
        ] {
            assert!(
                matches!(parse(text.as_bytes()), Err(CodecError::Decode(_))),
                "{text}"
            );
        }
    }
}
