// Public API in this module is consumed by future commands (FG-3/FG-4); suppress premature dead_code warnings.
#![allow(dead_code)]

use crate::trakt_client::{HttpResponse, TraktHttpClient};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn post<T: Serialize>(
    client: &dyn TraktHttpClient,
    url: &str,
    body: &T,
) -> Result<HttpResponse, String> {
    let json = serde_json::to_string(body).map_err(|e| e.to_string())?;
    client.post_json(url, &json)
}

#[derive(Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Serialize)]
struct DeviceTokenRequest<'a> {
    code: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
}

#[derive(Deserialize)]
struct TokenApiResponse {
    access_token: String,
    refresh_token: String,
    created_at: u64,
    expires_in: u64,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
    redirect_uri: &'a str,
    grant_type: &'a str,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub created_at: u64,
    pub expires_in: u64,
}

impl StoredTokens {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Treat as expired when within 5 minutes of expiry to allow proactive refresh.
        now + 300 >= self.created_at + self.expires_in
    }
}

fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tokens.json")
}

fn save_tokens(data_dir: &Path, tokens: &StoredTokens) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;

    let dest = token_path(data_dir);
    let tmp = data_dir.join("tokens.json.tmp");

    let json = serde_json::to_string_pretty(tokens).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, &json).map_err(|e| format!("failed to write token file: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("failed to set token file permissions: {e}"))?;
    }

    std::fs::rename(&tmp, &dest).map_err(|e| format!("failed to rename token file: {e}"))?;
    Ok(())
}

pub fn load_tokens(data_dir: &Path) -> Option<StoredTokens> {
    let content = std::fs::read_to_string(token_path(data_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn run_device_flow(
    client: &dyn TraktHttpClient,
    client_id: &str,
    client_secret: &str,
    data_dir: &Path,
) -> Result<StoredTokens, String> {
    run_device_flow_inner(client, client_id, client_secret, data_dir, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn run_device_flow_inner(
    client: &dyn TraktHttpClient,
    client_id: &str,
    client_secret: &str,
    data_dir: &Path,
    sleep: &dyn Fn(u64),
) -> Result<StoredTokens, String> {
    let resp = post(
        client,
        "https://api.trakt.tv/oauth/device/code",
        &DeviceCodeRequest { client_id },
    )?;
    if resp.status != 200 {
        return Err(format!("failed to get device code: HTTP {}", resp.status));
    }

    let dc: DeviceCodeResponse = serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse device code response: {e}"))?;

    eprintln!();
    eprintln!("  Visit:      {}", dc.verification_url);
    eprintln!("  Enter code: {}", dc.user_code);
    eprintln!();
    eprintln!("Waiting for authorization...");

    let start = SystemTime::now();
    let timeout = Duration::from_secs(dc.expires_in);
    let mut interval = dc.interval;

    loop {
        sleep(interval);

        if start.elapsed().unwrap_or(timeout) >= timeout {
            return Err("authorization timed out".to_string());
        }

        let resp = post(
            client,
            "https://api.trakt.tv/oauth/device/token",
            &DeviceTokenRequest {
                code: &dc.device_code,
                client_id,
                client_secret,
            },
        )?;

        match resp.status {
            200 => {
                let tr: TokenApiResponse = serde_json::from_str(&resp.body)
                    .map_err(|e| format!("failed to parse token response: {e}"))?;
                let tokens = StoredTokens {
                    access_token: tr.access_token,
                    refresh_token: tr.refresh_token,
                    created_at: tr.created_at,
                    expires_in: tr.expires_in,
                };
                save_tokens(data_dir, &tokens)?;
                return Ok(tokens);
            }
            400 => continue,
            404 => return Err("device code not found".to_string()),
            409 => return Err("device code already used".to_string()),
            410 => return Err("device code expired".to_string()),
            418 => return Err("authorization denied by user".to_string()),
            429 => {
                interval = interval.saturating_add(5);
                eprintln!("Rate-limited; increasing polling interval.");
            }
            s => return Err(format!("unexpected status from token endpoint: {s}")),
        }
    }
}

fn refresh_tokens(
    client: &dyn TraktHttpClient,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
    data_dir: &Path,
) -> Result<StoredTokens, String> {
    let resp = post(
        client,
        "https://api.trakt.tv/oauth/token",
        &RefreshRequest {
            refresh_token,
            client_id,
            client_secret,
            redirect_uri: "urn:ietf:wg:oauth:2.0:oob",
            grant_type: "refresh_token",
        },
    )?;

    if resp.status != 200 {
        return Err(format!("token refresh failed: HTTP {}", resp.status));
    }

    let tr: TokenApiResponse = serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse refresh response: {e}"))?;

    let tokens = StoredTokens {
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        created_at: tr.created_at,
        expires_in: tr.expires_in,
    };
    save_tokens(data_dir, &tokens)?;
    Ok(tokens)
}

/// Returns a valid access token, transparently refreshing if expired.
/// Call this before every authenticated Trakt API request.
pub fn get_valid_token(
    client: &dyn TraktHttpClient,
    client_id: &str,
    client_secret: &str,
    data_dir: &Path,
) -> Result<String, String> {
    let tokens = load_tokens(data_dir)
        .ok_or_else(|| "not authenticated — run `trakt-letterboxd auth` first".to_string())?;

    if tokens.is_expired() {
        let refreshed = refresh_tokens(
            client,
            client_id,
            client_secret,
            &tokens.refresh_token,
            data_dir,
        )?;
        Ok(refreshed.access_token)
    } else {
        Ok(tokens.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct MockClient {
        responses: Mutex<VecDeque<(u16, String)>>,
    }

    impl MockClient {
        fn new(responses: Vec<(u16, String)>) -> Self {
            MockClient {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    impl TraktHttpClient for MockClient {
        fn post_json(&self, _url: &str, _body: &str) -> Result<HttpResponse, String> {
            let mut q = self.responses.lock().unwrap();
            let (status, body) = q
                .pop_front()
                .ok_or_else(|| "no more mock responses".to_string())?;
            Ok(HttpResponse { status, body })
        }
    }

    fn device_code_json(interval: u64) -> String {
        format!(
            r#"{{"device_code":"dc123","user_code":"ABCD-1234","verification_url":"https://trakt.tv/activate","expires_in":600,"interval":{interval}}}"#
        )
    }

    fn token_json(access: &str, refresh: &str) -> String {
        format!(
            r#"{{"access_token":"{access}","refresh_token":"{refresh}","token_type":"Bearer","expires_in":7776000,"scope":"public","created_at":1000000}}"#
        )
    }

    #[test]
    fn device_flow_happy_path() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (200, token_json("access1", "refresh1")),
        ]);
        let tokens = run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        assert_eq!(tokens.access_token, "access1");
        assert_eq!(tokens.refresh_token, "refresh1");
        assert_eq!(load_tokens(dir.path()).unwrap().access_token, "access1");
    }

    #[test]
    fn device_flow_pending_then_success() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (400, "{}".to_string()),
            (400, "{}".to_string()),
            (200, token_json("access2", "refresh2")),
        ]);
        let tokens = run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        assert_eq!(tokens.access_token, "access2");
    }

    #[test]
    fn device_flow_denied() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![(200, device_code_json(0)), (418, "{}".to_string())]);
        let err = run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap_err();
        assert!(err.contains("denied"), "expected denial error, got: {err}");
    }

    #[test]
    fn device_flow_expired_code() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![(200, device_code_json(0)), (410, "{}".to_string())]);
        let err = run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap_err();
        assert!(err.contains("expired"), "expected expiry error, got: {err}");
    }

    #[test]
    fn device_flow_slow_down_then_success() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (429, "{}".to_string()),
            (200, token_json("access3", "refresh3")),
        ]);
        let tokens = run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        assert_eq!(tokens.access_token, "access3");
    }

    #[test]
    fn device_flow_overwrites_existing_tokens() {
        let dir = TempDir::new().unwrap();
        let old = StoredTokens {
            access_token: "old".to_string(),
            refresh_token: "old_ref".to_string(),
            created_at: 0,
            expires_in: 1,
        };
        save_tokens(dir.path(), &old).unwrap();

        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (200, token_json("new_access", "new_refresh")),
        ]);
        run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        assert_eq!(load_tokens(dir.path()).unwrap().access_token, "new_access");
    }

    #[test]
    fn get_valid_token_returns_existing_when_fresh() {
        let dir = TempDir::new().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let tokens = StoredTokens {
            access_token: "valid_token".to_string(),
            refresh_token: "refresh".to_string(),
            created_at: now,
            expires_in: 7776000,
        };
        save_tokens(dir.path(), &tokens).unwrap();
        let client = MockClient::new(vec![]);
        assert_eq!(
            get_valid_token(&client, "cid", "csec", dir.path()).unwrap(),
            "valid_token"
        );
    }

    #[test]
    fn get_valid_token_refreshes_expired() {
        let dir = TempDir::new().unwrap();
        let tokens = StoredTokens {
            access_token: "expired".to_string(),
            refresh_token: "old_refresh".to_string(),
            created_at: 0,
            expires_in: 1,
        };
        save_tokens(dir.path(), &tokens).unwrap();
        let client = MockClient::new(vec![(200, token_json("fresh_access", "new_refresh"))]);
        assert_eq!(
            get_valid_token(&client, "cid", "csec", dir.path()).unwrap(),
            "fresh_access"
        );
    }

    #[test]
    fn get_valid_token_errors_without_auth() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![]);
        let err = get_valid_token(&client, "cid", "csec", dir.path()).unwrap_err();
        assert!(err.contains("auth"), "expected auth error, got: {err}");
    }

    #[test]
    fn tokens_roundtrip() {
        let dir = TempDir::new().unwrap();
        let tokens = StoredTokens {
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            created_at: 12345,
            expires_in: 67890,
        };
        save_tokens(dir.path(), &tokens).unwrap();
        let loaded = load_tokens(dir.path()).unwrap();
        assert_eq!(loaded.access_token, "acc");
        assert_eq!(loaded.refresh_token, "ref");
        assert_eq!(loaded.created_at, 12345);
        assert_eq!(loaded.expires_in, 67890);
    }

    #[test]
    #[cfg(unix)]
    fn token_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let tokens = StoredTokens {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            created_at: 0,
            expires_in: 1,
        };
        save_tokens(dir.path(), &tokens).unwrap();
        let mode = std::fs::metadata(dir.path().join("tokens.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn is_expired_past_expiry() {
        let tokens = StoredTokens {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            created_at: 0,
            expires_in: 1,
        };
        assert!(tokens.is_expired());
    }

    #[test]
    fn is_not_expired_far_from_expiry() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let tokens = StoredTokens {
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            created_at: now,
            expires_in: 7776000,
        };
        assert!(!tokens.is_expired());
    }

    #[test]
    #[cfg(unix)]
    fn device_flow_saves_token_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (200, token_json("access_perm", "refresh_perm")),
        ]);
        run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        let mode = std::fs::metadata(dir.path().join("tokens.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "tokens.json written by device flow must have mode 0600"
        );
    }

    #[test]
    fn refresh_persists_new_token_to_disk() {
        let dir = TempDir::new().unwrap();
        let expired = StoredTokens {
            access_token: "expired_access".to_string(),
            refresh_token: "old_refresh".to_string(),
            created_at: 0,
            expires_in: 1,
        };
        save_tokens(dir.path(), &expired).unwrap();
        let client = MockClient::new(vec![(200, token_json("fresh_access", "fresh_refresh"))]);
        get_valid_token(&client, "cid", "csec", dir.path()).unwrap();
        let on_disk = load_tokens(dir.path()).unwrap();
        assert_eq!(on_disk.access_token, "fresh_access");
        assert_eq!(on_disk.refresh_token, "fresh_refresh");
    }

    #[test]
    fn no_stale_temp_file_after_device_flow() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(vec![
            (200, device_code_json(0)),
            (200, token_json("acc", "ref")),
        ]);
        run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap();
        assert!(
            !dir.path().join("tokens.json.tmp").exists(),
            "temp file must not remain after successful device flow"
        );
    }

    #[test]
    fn device_flow_times_out_without_infinite_loop() {
        // expires_in:0 means timeout=Duration::ZERO; any elapsed time satisfies the
        // check, so the loop returns "timed out" after the very first sleep without
        // ever polling the token endpoint — proving no infinite-loop is possible.
        let dir = TempDir::new().unwrap();
        let dc_json = r#"{"device_code":"dc123","user_code":"ABCD","verification_url":"https://trakt.tv/activate","expires_in":0,"interval":0}"#.to_string();
        let client = MockClient::new(vec![(200, dc_json)]);
        let err =
            run_device_flow_inner(&client, "cid", "csec", dir.path(), &|_| {}).unwrap_err();
        assert!(err.contains("timed out"), "expected timeout error, got: {err}");
    }

    #[test]
    fn malformed_token_file_triggers_auth_error_not_panic() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("tokens.json"), b"not json {{{{").unwrap();
        let client = MockClient::new(vec![]);
        let err = get_valid_token(&client, "cid", "csec", dir.path()).unwrap_err();
        assert!(
            err.contains("auth"),
            "expected auth error for malformed token file, got: {err}"
        );
    }
}
