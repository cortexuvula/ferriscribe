//! Read an HTTP error response's body for diagnostic logging without losing
//! the read-error context. Used by every HTTP client in this workspace.

use reqwest::Response;

/// Attempt to read up to `max_chars` characters of the response body. On
/// read failure, returns a "(could not read body: <error>)" placeholder so
/// the caller's downstream error message still has a useful tail. Truncates
/// to bound log line length.
pub async fn read_error_body(resp: Response, max_chars: usize) -> String {
    match resp.text().await {
        Ok(body) => body.chars().take(max_chars).collect(),
        Err(e) => format!("(could not read body: {e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(body: &'static str) -> Response {
        // reqwest::Response is constructed from an http::Response<Body>;
        // String implements Into<reqwest::Body>, satisfying the From bound.
        let http_resp = http::Response::builder()
            .status(500)
            .body(body.to_string())
            .unwrap();
        Response::from(http_resp)
    }

    #[tokio::test]
    async fn returns_body_when_under_limit() {
        let resp = make_response("hello");
        let result = read_error_body(resp, 200).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn truncates_body_when_over_limit() {
        let resp = make_response("hello world");
        let result = read_error_body(resp, 5).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn truncates_at_unicode_codepoint_boundary() {
        // 5 codepoints "héllo" — must not split a multi-byte boundary.
        let resp = make_response("héllo world");
        let result = read_error_body(resp, 5).await;
        assert_eq!(result, "héllo");
    }
}
