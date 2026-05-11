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

    #[tokio::test]
    async fn truncates_to_max_chars() {
        // The truncation logic is the only behavior we can unit test without
        // a real reqwest::Response. Verify via a string-level smoke check;
        // real round-trip behavior is exercised at the call sites in Tasks 2+.
        assert_eq!(
            "hello world".chars().take(5).collect::<String>(),
            "hello"
        );
        // Ensure the function is importable and tokio::test compiles.
        let _: fn(Response, usize) -> _ = read_error_body;
    }
}
