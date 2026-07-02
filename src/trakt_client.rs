use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, USER_AGENT};

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait TraktHttpClient {
    fn post_json(&self, url: &str, body: &str) -> Result<HttpResponse, String>;
}

pub struct ReqwestClient {
    client: reqwest::blocking::Client,
}

impl ReqwestClient {
    pub fn new(client_id: &str) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert("trakt-api-version", HeaderValue::from_static("2"));
        headers.insert(
            "trakt-api-key",
            HeaderValue::from_str(client_id).expect("trakt client_id must be valid ASCII"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("trakt-letterboxd/", env!("CARGO_PKG_VERSION"))),
        );
        ReqwestClient {
            client: reqwest::blocking::Client::builder()
                .default_headers(headers)
                .build()
                .expect("failed to build HTTP client"),
        }
    }
}

impl TraktHttpClient for ReqwestClient {
    fn post_json(&self, url: &str, body: &str) -> Result<HttpResponse, String> {
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|e| format!("failed to read response body: {e}"))?;

        Ok(HttpResponse { status, body })
    }
}
