use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub code: String,
    pub payload: Value,
}

impl Frame {
    pub fn new(code: &str, payload: Value) -> Self {
        Self {
            code: code.to_string(),
            payload,
        }
    }

    pub fn encode(&self) -> String {
        format!("{}{}\n", self.code, self.payload)
    }

    pub fn req_id(&self) -> Option<&str> {
        self.payload
            .get("req_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn is_error(&self) -> bool {
        self.code == "EROR" || self.payload.get("error").and_then(Value::as_bool) == Some(true)
    }

    pub fn decode_all(text: &str) -> Vec<Frame> {
        text.split('\n')
            .filter(|line| line.len() >= 4)
            .filter_map(|line| {
                let code = line.get(..4)?;
                let payload = serde_json::from_str(&line[4..]).ok()?;
                Some(Frame::new(code, payload))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_and_splits() {
        let frame = Frame::new("READ", json!({"channel_url":"c","req_id":"rq-1"}));
        let encoded = frame.encode();
        assert!(encoded.starts_with("READ{"));
        assert!(encoded.ends_with('\n'));
        let both = format!("{encoded}PONG{{\"id\":1}}\n");
        let frames = Frame::decode_all(&both);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].req_id(), Some("rq-1"));
        assert_eq!(frames[1].code, "PONG");
    }

    #[test]
    fn skips_garbage_and_detects_errors() {
        assert!(Frame::decode_all("MESG{not json\n\n").is_empty());
        let err = &Frame::decode_all(r#"LOGI{"error":true,"code":400302,"message":"x"}"#)[0];
        assert!(err.is_error());
    }
}
