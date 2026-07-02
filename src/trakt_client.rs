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
    pub fn new() -> Self {
        ReqwestClient {
            client: reqwest::blocking::Client::new(),
        }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TraktHttpClient for ReqwestClient {
    fn post_json(&self, url: &str, body: &str) -> Result<HttpResponse, String> {
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("trakt-api-version", "2")
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
