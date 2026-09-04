//! Reading OTLP attributes.
//!
//! Claude Code's event attributes are a flat `Vec<KeyValue>` of typed values.
//! Every accessor here returns `Option`: an attribute that is absent, or present
//! with an unexpected type, yields `None` rather than an error. Attribute sets
//! change between Claude Code versions, and a receiver that rejects an event it
//! only partly understands would discard exactly the audit record it exists to
//! keep.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value::Value};

/// A borrowed view over one event's attributes.
#[derive(Debug, Clone, Copy)]
pub struct Attrs<'a>(pub &'a [KeyValue]);

impl<'a> Attrs<'a> {
    fn raw(&self, key: &str) -> Option<&'a Value> {
        self.0
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| kv.value.as_ref())
            .and_then(|v: &AnyValue| v.value.as_ref())
    }

    /// A string attribute.
    #[must_use]
    pub fn str(&self, key: &str) -> Option<&'a str> {
        match self.raw(key)? {
            Value::StringValue(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// A string attribute, owned.
    #[must_use]
    pub fn string(&self, key: &str) -> Option<String> {
        self.str(key).map(str::to_string)
    }

    /// An integer attribute.
    ///
    /// Exporters are inconsistent about numeric types, so a double or a numeric
    /// string is accepted too — Claude Code sends `status_code` as a string and
    /// `duration_ms` as an int, and both are worth having.
    #[must_use]
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.raw(key)? {
            Value::IntValue(i) => Some(*i),
            #[allow(clippy::cast_possible_truncation)]
            Value::DoubleValue(d) => Some(*d as i64),
            Value::StringValue(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// A floating-point attribute.
    #[must_use]
    pub fn float(&self, key: &str) -> Option<f64> {
        match self.raw(key)? {
            Value::DoubleValue(d) => Some(*d),
            #[allow(clippy::cast_precision_loss)]
            Value::IntValue(i) => Some(*i as f64),
            Value::StringValue(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// A boolean attribute, accepting the string forms exporters also emit.
    #[must_use]
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.raw(key)? {
            Value::BoolValue(b) => Some(*b),
            Value::StringValue(s) => match s.as_str() {
                "true" | "True" | "1" => Some(true),
                "false" | "False" | "0" => Some(false),
                _ => None,
            },
            Value::IntValue(i) => Some(*i != 0),
            _ => None,
        }
    }

    /// A string-array attribute, such as `workspace.host_paths`.
    #[must_use]
    pub fn str_array(&self, key: &str) -> Option<Vec<String>> {
        match self.raw(key)? {
            Value::ArrayValue(a) => Some(
                a.values
                    .iter()
                    .filter_map(|v| match v.value.as_ref()? {
                        Value::StringValue(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            Value::StringValue(s) => Some(vec![s.clone()]),
            _ => None,
        }
    }

    /// The first value of a string-array attribute.
    #[must_use]
    pub fn first_of_array(&self, key: &str) -> Option<String> {
        self.str_array(key)?.into_iter().next()
    }

    /// A JSON-encoded attribute, parsed.
    ///
    /// `tool_parameters` arrives as a JSON string rather than nested attributes,
    /// and carries the MCP server and tool names among other things.
    #[must_use]
    pub fn json(&self, key: &str) -> Option<serde_json::Value> {
        serde_json::from_str(self.str(key)?).ok()
    }

    /// Whether any attribute with this key is present.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.0.iter().any(|kv| kv.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::ArrayValue;

    fn kv(key: &str, value: Value) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue { value: Some(value) }),
            ..KeyValue::default()
        }
    }

    fn sample() -> Vec<KeyValue> {
        vec![
            kv("tool_name", Value::StringValue("Bash".into())),
            kv("duration_ms", Value::IntValue(42)),
            kv("success", Value::BoolValue(true)),
            kv("cost", Value::DoubleValue(0.5)),
            kv("status_code", Value::StringValue("404".into())),
            kv("flag_str", Value::StringValue("true".into())),
            kv(
                "workspace.host_paths",
                Value::ArrayValue(ArrayValue {
                    values: vec![
                        AnyValue {
                            value: Some(Value::StringValue("/work/a".into())),
                        },
                        AnyValue {
                            value: Some(Value::StringValue("/work/b".into())),
                        },
                    ],
                }),
            ),
            kv(
                "tool_parameters",
                Value::StringValue(r#"{"bash_command":"ls"}"#.into()),
            ),
            kv("empty", Value::StringValue(String::new())),
        ]
    }

    #[test]
    fn reads_each_value_type() {
        let a = Attrs(&sample()[..]);
        assert_eq!(a.str("tool_name"), Some("Bash"));
        assert_eq!(a.int("duration_ms"), Some(42));
        assert_eq!(a.bool("success"), Some(true));
        assert!((a.float("cost").expect("cost") - 0.5).abs() < f64::EPSILON);
        assert_eq!(a.str_array("workspace.host_paths").expect("paths").len(), 2);
        assert_eq!(
            a.first_of_array("workspace.host_paths").as_deref(),
            Some("/work/a")
        );
    }

    /// Exporters are inconsistent about numeric and boolean types; the audit
    /// value of the field does not depend on which one arrived.
    #[test]
    fn coerces_across_the_types_exporters_actually_send() {
        let a = Attrs(&sample()[..]);
        assert_eq!(a.int("status_code"), Some(404), "numeric string");
        assert_eq!(a.int("cost"), Some(0), "double truncated to int");
        assert_eq!(a.bool("flag_str"), Some(true), "string boolean");
        assert!(a.float("duration_ms").is_some(), "int as float");
    }

    #[test]
    fn json_attributes_are_parsed() {
        let a = Attrs(&sample()[..]);
        let params = a.json("tool_parameters").expect("parsed");
        assert_eq!(params["bash_command"], "ls");
        assert!(
            a.json("tool_name").is_none(),
            "not JSON, so None rather than an error"
        );
    }

    /// The property the module is built on: nothing here can fail an ingest.
    #[test]
    fn absent_and_mistyped_attributes_yield_none() {
        let a = Attrs(&sample()[..]);
        assert_eq!(a.str("nope"), None);
        assert_eq!(a.int("tool_name"), None, "not a number");
        assert_eq!(a.bool("tool_name"), None);
        assert_eq!(a.str_array("duration_ms"), None);
        assert!(!a.has("nope"));
        assert!(a.has("empty"));
        assert_eq!(a.str("empty"), Some(""));
    }
}
