use crate::{Codec, CodecError};
use c2_model::C2Message;
use serde_json::Value;

pub struct MtfXmlCodec;

const ROOT_OPEN: &str = "<c2message>";
const ROOT_CLOSE: &str = "</c2message>";

impl Codec for MtfXmlCodec {
    fn wire_format(&self) -> &'static str {
        "mtf-xml"
    }

    fn encode(&self, msg: &C2Message) -> Result<Vec<u8>, CodecError> {
        let value = serde_json::to_value(msg).map_err(|e| CodecError::Encode(e.to_string()))?;
        Ok(value_to_xml(&value)?.into_bytes())
    }

    fn decode(&self, bytes: &[u8]) -> Result<C2Message, CodecError> {
        let xml = std::str::from_utf8(bytes).map_err(|e| CodecError::Decode(e.to_string()))?;
        let value = xml_to_value(xml)?;
        serde_json::from_value(value).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

fn value_to_xml(value: &Value) -> Result<String, CodecError> {
    let obj = value
        .as_object()
        .ok_or_else(|| CodecError::Encode("top-level message is not an object".to_string()))?;
    let mut out = String::from(ROOT_OPEN);
    for (key, field) in obj {
        let text = match field {
            Value::String(s) => escape(s),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            other => {
                return Err(CodecError::Encode(format!(
                    "field {key} has unsupported value {other}"
                )))
            }
        };
        out.push('<');
        out.push_str(key);
        out.push('>');
        out.push_str(&text);
        out.push_str("</");
        out.push_str(key);
        out.push('>');
    }
    out.push_str(ROOT_CLOSE);
    Ok(out)
}

fn xml_to_value(xml: &str) -> Result<Value, CodecError> {
    let inner = xml
        .trim()
        .strip_prefix(ROOT_OPEN)
        .and_then(|s| s.strip_suffix(ROOT_CLOSE))
        .ok_or_else(|| CodecError::Decode("missing c2message root element".to_string()))?;

    let mut map = serde_json::Map::new();
    let mut rest = inner;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            return Err(CodecError::Decode("expected element start".to_string()));
        }
        let close = rest
            .find('>')
            .ok_or_else(|| CodecError::Decode("unterminated opening tag".to_string()))?;
        let tag = &rest[1..close];
        let after_open = &rest[close + 1..];
        let end_tag = format!("</{tag}>");
        let end = after_open
            .find(&end_tag)
            .ok_or_else(|| CodecError::Decode(format!("missing closing tag for {tag}")))?;
        let raw = &after_open[..end];
        map.insert(tag.to_string(), infer(&unescape(raw)));
        rest = &after_open[end + end_tag.len()..];
    }
    Ok(Value::Object(map))
}

fn infer(text: &str) -> Value {
    if let Ok(u) = text.parse::<u64>() {
        return Value::from(u);
    }
    if let Ok(i) = text.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = text.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(f) {
            return Value::Number(n);
        }
    }
    Value::String(text.to_string())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use c2_model::{ContactReport, Domain, FunctionalService, Sitrep};

    #[test]
    fn contact_report_round_trips_with_special_chars_and_floats() {
        let msg = C2Message::ContactReport(ContactReport {
            id: "C-1001".to_string(),
            reporting_unit: "UAV-7".to_string(),
            domain: Domain::Air,
            service: FunctionalService::Intelligence,
            observed_at: "2026-09-23T14:30:00Z".to_string(),
            latitude: 49.2827,
            longitude: -123.1207,
            description: "Vehicle <light> & stationary".to_string(),
        });
        let codec = MtfXmlCodec;
        let bytes = codec.encode(&msg).unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).contains("<message_type>ContactReport</message_type>")
        );
        assert_eq!(codec.decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn sitrep_round_trips_with_integer_field() {
        let msg = C2Message::Sitrep(Sitrep {
            id: "S-9".to_string(),
            reporting_unit: "Coy-A".to_string(),
            domain: Domain::Land,
            service: FunctionalService::CommandAndControl,
            reported_at: "2026-09-23T16:00:00Z".to_string(),
            summary: "Holding position".to_string(),
            personnel_effective: 118,
        });
        let codec = MtfXmlCodec;
        let bytes = codec.encode(&msg).unwrap();
        assert_eq!(codec.decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn missing_root_is_decode_error() {
        let codec = MtfXmlCodec;
        assert!(matches!(
            codec.decode(b"<wrong></wrong>"),
            Err(CodecError::Decode(_))
        ));
    }
}
