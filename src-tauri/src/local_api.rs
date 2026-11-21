use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

use crate::activity::{ActivityManager, Priority};

const MAX_BODY_BYTES: usize = 8 * 1024;
const MAX_EVENTS_PER_SEC: usize = 30;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEvent {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub action_panel: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiStatus {
    pub enabled: bool,
    pub listening: bool,
    pub port: u16,
    pub error: String,
}

struct RunningApi {
    port: u16,
    token: String,
    stop_tx: oneshot::Sender<()>,
}

#[derive(Default)]
pub struct LocalApiState {
    running: Mutex<Option<RunningApi>>,
    last_error: Mutex<String>,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub enum ParsedApiRequest {
    Health,
    PostEvent(IncomingEvent),
}

pub fn validate_http_request(
    raw: &[u8],
    expected_token: &str,
) -> Result<ParsedApiRequest, (u16, &'static str)> {
    let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err((400, "Incomplete HTTP request"));
    };
    let head =
        std::str::from_utf8(&raw[..header_end]).map_err(|_| (400, "Invalid UTF-8 headers"))?;
    let mut lines = head.split("\r\n");
    let req_line = lines.next().ok_or((400, "Missing request line"))?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut auth_header = None;
    let mut content_length: usize = 0;
    for line in lines {
        if let Some((name, val)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("authorization") {
                auth_header = Some(val.trim());
            } else if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = val
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| (400, "Invalid Content-Length"))?;
            }
        }
    }

    if content_length > MAX_BODY_BYTES {
        return Err((413, "Payload too large"));
    }

    let expected_header = format!("Bearer {expected_token}");
    let Some(provided) = auth_header else {
        return Err((401, "Missing Authorization header"));
    };
    if !constant_time_eq(provided.as_bytes(), expected_header.as_bytes()) {
        return Err((401, "Unauthorized"));
    }

    match (method, path) {
        ("GET", "/v1/health") => Ok(ParsedApiRequest::Health),
        ("POST", "/v1/events") => {
            let body_slice = &raw[header_end + 4..];
            if body_slice.len() > MAX_BODY_BYTES {
                return Err((413, "Payload too large"));
            }
            let body = if content_length > 0 && body_slice.len() >= content_length {
                &body_slice[..content_length]
            } else {
                body_slice
            };
            let ev: IncomingEvent =
                serde_json::from_slice(body).map_err(|_| (400, "Invalid JSON event"))?;
            if ev.title.trim().is_empty() {
                return Err((400, "Event title is required"));
            }
            Ok(ParsedApiRequest::PostEvent(ev))
        }
        _ => Err((404, "Not found")),
    }
}

fn format_json_response(code: u16, reason: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

pub async fn sync_config(app: AppHandle, enabled: bool, port: u16, token: String) {
    let state = app.state::<LocalApiState>();
    {
        let mut guard = state.running.lock().unwrap();
        if let Some(existing) = guard.as_ref() {
            if enabled && existing.port == port && existing.token == token {
                return;
            }
        }
        if let Some(old) = guard.take() {
            let _ = old.stop_tx.send(());
        }
        *state.last_error.lock().unwrap() = String::new();
    }

    if !enabled || token.trim().is_empty() {
        return;
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(err) => {
            *state.last_error.lock().unwrap() = format!("Port {port} unavailable: {err}");
            return;
        }
    };

    let (stop_tx, mut stop_rx) = oneshot::channel();
    *state.running.lock().unwrap() = Some(RunningApi {
        port,
        token: token.clone(),
        stop_tx,
    });

    let rate_state = Arc::new(Mutex::new((Instant::now(), 0usize)));
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                accepted = listener.accept() => {
                    let Ok((mut socket, _)) = accepted else { continue; };
                    let app_clone = app.clone();
                    let tok = token.clone();
                    let rl = rate_state.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut buf = Vec::with_capacity(2048);
                        let mut tmp = [0u8; 2048];
                        let deadline = tokio::time::sleep(Duration::from_millis(800));
                        tokio::pin!(deadline);
                        loop {
                            tokio::select! {
                                _ = &mut deadline => break,
                                read_res = socket.read(&mut tmp) => {
                                    let Ok(n) = read_res else { return; };
                                    if n == 0 { break; }
                                    if buf.len() + n > MAX_BODY_BYTES + 2048 {
                                        let resp = format_json_response(413, "Payload Too Large", r#"{"error":"Payload too large"}"#);
                                        let _ = socket.write_all(&resp).await;
                                        return;
                                    }
                                    buf.extend_from_slice(&tmp[..n]);
                                    if let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                        let head = String::from_utf8_lossy(&buf[..hdr_end]);
                                        let cl: usize = head
                                            .lines()
                                            .find_map(|l| {
                                                let (k, v) = l.split_once(':')?;
                                                k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse().ok()).flatten()
                                            })
                                            .unwrap_or(0);
                                        if buf.len() >= hdr_end + 4 + cl {
                                            break;
                                        }
                                    }
                                }
                            }
                        }

                        let rate_limited = {
                            let mut rg = rl.lock().unwrap();
                            if rg.0.elapsed() >= Duration::from_secs(1) {
                                *rg = (Instant::now(), 0);
                            }
                            rg.1 += 1;
                            rg.1 > MAX_EVENTS_PER_SEC
                        };
                        if rate_limited {
                            let resp = format_json_response(
                                429,
                                "Too Many Requests",
                                r#"{"error":"Rate limit exceeded"}"#,
                            );
                            let _ = socket.write_all(&resp).await;
                            return;
                        }

                        let resp = match validate_http_request(&buf, &tok) {
                            Ok(ParsedApiRequest::Health) => {
                                format_json_response(200, "OK", r#"{"ok":true,"service":"peek"}"#)
                            }
                            Ok(ParsedApiRequest::PostEvent(ev)) => {
                                let source = if ev.source.trim().is_empty() {
                                    "Local API".to_string()
                                } else {
                                    ev.source.trim().to_string()
                                };
                                let ttl_ms = ev.ttl_seconds.unwrap_or(4).clamp(1, 30) * 1000;
                                app_clone.state::<ActivityManager>().push_activity(
                                    source,
                                    ev.title.trim().to_string(),
                                    ev.body.trim().to_string(),
                                    ev.priority.unwrap_or(Priority::Normal),
                                    ttl_ms,
                                    ev.action_panel.as_deref(),
                                    ev.sensitive,
                                    true,
                                );
                                format_json_response(200, "OK", r#"{"ok":true}"#)
                            }
                            Err((code, msg)) => {
                                let body = serde_json::json!({ "error": msg }).to_string();
                                let reason = match code {
                                    401 => "Unauthorized",
                                    404 => "Not Found",
                                    413 => "Payload Too Large",
                                    _ => "Bad Request",
                                };
                                format_json_response(code, reason, &body)
                            }
                        };
                        let _ = socket.write_all(&resp).await;
                    });
                }
            }
        }
    });
}

#[tauri::command]
pub fn get_local_api_status(
    prefs: State<'_, crate::preferences::PreferencesState>,
    state: State<'_, LocalApiState>,
) -> LocalApiStatus {
    let p = prefs.0.lock().unwrap();
    let listening = state.running.lock().unwrap().is_some();
    let error = state.last_error.lock().unwrap().clone();
    LocalApiStatus {
        enabled: p.local_api_enabled,
        listening,
        port: p.local_api_port,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_auth_and_payload_bounds() {
        let req_unauth = b"POST /v1/events HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 14\r\n\r\n{\"title\":\"Hi\"}";
        assert_eq!(
            validate_http_request(req_unauth, "secret").err().unwrap().0,
            401
        );

        let req_bad_token = b"POST /v1/events HTTP/1.1\r\nAuthorization: Bearer wrong\r\nContent-Length: 14\r\n\r\n{\"title\":\"Hi\"}";
        assert_eq!(
            validate_http_request(req_bad_token, "secret")
                .err()
                .unwrap()
                .0,
            401
        );

        let req_ok = b"POST /v1/events HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 14\r\n\r\n{\"title\":\"Hi\"}";
        assert!(matches!(
            validate_http_request(req_ok, "secret").unwrap(),
            ParsedApiRequest::PostEvent(_)
        ));

        let req_bad_json = b"POST /v1/events HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(
            validate_http_request(req_bad_json, "secret")
                .err()
                .unwrap()
                .0,
            400
        );

        let req_big = b"POST /v1/events HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 99999\r\n\r\n{}";
        assert_eq!(
            validate_http_request(req_big, "secret").err().unwrap().0,
            413
        );
    }
}
