//! Serde shapes for the CDP `Network.*` event params the monitor consumes.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestWillBeSent {
    pub request_id: String,
    pub request: CdpRequest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CdpRequest {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub post_data: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseReceived {
    pub request_id: String,
    pub response: CdpResponse,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CdpResponse {
    pub status: u16,
    #[serde(default)]
    pub status_text: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub mime_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestIdOnly {
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadingFailed {
    pub request_id: String,
    #[serde(default)]
    pub error_text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSocketCreated {
    pub request_id: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebSocketFrameEvent {
    pub request_id: String,
    pub response: WsFrame,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WsFrame {
    pub opcode: u8,
    #[serde(default)]
    pub payload_data: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventSourceMessage {
    pub request_id: String,
    #[serde(default)]
    pub event_name: String,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub data: String,
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request_will_be_sent() {
        let v = json!({
            "requestId": "1",
            "request": {
                "url": "https://x/a",
                "method": "GET",
                "headers": { "A": "b" }
            }
        });
        let p: RequestWillBeSent = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "1");
        assert_eq!(p.request.url, "https://x/a");
        assert_eq!(p.request.method, "GET");
        assert_eq!(p.request.headers.get("A").map(String::as_str), Some("b"));
        assert!(p.request.post_data.is_none());
    }

    #[test]
    fn parses_request_will_be_sent_with_post_data() {
        let v = json!({
            "requestId": "2",
            "request": {
                "url": "https://x/b",
                "method": "POST",
                "postData": "hello=world"
            }
        });
        let p: RequestWillBeSent = serde_json::from_value(v).unwrap();
        assert_eq!(p.request.method, "POST");
        assert_eq!(p.request.post_data.as_deref(), Some("hello=world"));
    }

    #[test]
    fn parses_response_received() {
        let v = json!({
            "requestId": "3",
            "response": {
                "status": 200,
                "statusText": "OK",
                "headers": { "Content-Type": "application/json" },
                "mimeType": "application/json"
            }
        });
        let p: ResponseReceived = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "3");
        assert_eq!(p.response.status, 200);
        assert_eq!(p.response.status_text, "OK");
        assert_eq!(p.response.mime_type, "application/json");
    }

    #[test]
    fn parses_response_received_defaults() {
        let v = json!({ "requestId": "4", "response": { "status": 404 } });
        let p: ResponseReceived = serde_json::from_value(v).unwrap();
        assert_eq!(p.response.status, 404);
        assert_eq!(p.response.status_text, "");
        assert_eq!(p.response.mime_type, "");
        assert!(p.response.headers.is_empty());
    }

    #[test]
    fn parses_request_id_only() {
        let v = json!({ "requestId": "5" });
        let p: RequestIdOnly = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "5");
    }

    #[test]
    fn parses_loading_failed() {
        let v = json!({ "requestId": "6", "errorText": "net::ERR_ABORTED" });
        let p: LoadingFailed = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "6");
        assert_eq!(p.error_text, "net::ERR_ABORTED");
    }

    #[test]
    fn parses_loading_failed_defaults() {
        let v = json!({ "requestId": "7" });
        let p: LoadingFailed = serde_json::from_value(v).unwrap();
        assert_eq!(p.error_text, "");
    }

    #[test]
    fn parses_web_socket_created() {
        let v = json!({ "requestId": "8", "url": "wss://echo.example.com" });
        let p: WebSocketCreated = serde_json::from_value(v).unwrap();
        assert_eq!(p.url, "wss://echo.example.com");
    }

    #[test]
    fn parses_ws_frame() {
        let v = json!({ "requestId": "2", "response": { "opcode": 1, "payloadData": "hi" } });
        let p: WebSocketFrameEvent = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "2");
        assert_eq!(p.response.opcode, 1);
        assert_eq!(p.response.payload_data, "hi");
    }

    #[test]
    fn parses_ws_frame_defaults() {
        let v = json!({ "requestId": "9", "response": { "opcode": 8 } });
        let p: WebSocketFrameEvent = serde_json::from_value(v).unwrap();
        assert_eq!(p.response.opcode, 8);
        assert_eq!(p.response.payload_data, "");
    }

    #[test]
    fn parses_event_source_message() {
        let v = json!({
            "requestId": "10",
            "eventName": "update",
            "eventId": "42",
            "data": "payload"
        });
        let p: EventSourceMessage = serde_json::from_value(v).unwrap();
        assert_eq!(p.request_id, "10");
        assert_eq!(p.event_name, "update");
        assert_eq!(p.event_id, "42");
        assert_eq!(p.data, "payload");
    }

    #[test]
    fn parses_event_source_message_defaults() {
        let v = json!({ "requestId": "11" });
        let p: EventSourceMessage = serde_json::from_value(v).unwrap();
        assert_eq!(p.event_name, "");
        assert_eq!(p.event_id, "");
        assert_eq!(p.data, "");
    }
}
