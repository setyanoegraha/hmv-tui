//! Authenticated HTTP session against hackmyvm.eu.
//!
//! The platform is an htmx app: forms are posted to path routes and answered
//! with an HTML fragment (or a redirect for full-page flows). Auth state is a
//! `session` cookie whose lifetime is two hours, so every response is checked
//! for a bounce back to `/login`.

use anyhow::{Context, Result};
use reqwest::Client;

use crate::config::ConfigManager;

pub const BASE_URL: &str = "https://hackmyvm.eu";
const LOGIN_PATH: &str = "/login";
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) ",
    "AppleWebKit/537.36 (KHTML, like Gecko) ",
    "Chrome/120.0.0.0 Safari/537.36 HMV-CLI/",
    env!("CARGO_PKG_VERSION")
);

/// One HTTP exchange after reqwest resolved redirects.
pub struct Fetched {
    /// Status of the final response.
    pub status: u16,
    /// Final URL path, e.g. `/dashboard`.
    pub path: String,
    pub text: String,
}

/// A logged-in session. Cloning is cheap: reqwest::Client shares one
/// connection pool and cookie jar internally.
#[derive(Clone)]
pub struct HmvSession {
    client: Client,
}

impl HmvSession {
    /// Runs a GET through the authenticated client.
    pub async fn get(&self, path: &str) -> Result<Fetched> {
        let resp = self
            .client
            .get(format!("{BASE_URL}{path}"))
            .send()
            .await
            .context("Connection error")?;
        collect(resp).await
    }

    /// Runs a form POST through the authenticated client.
    pub async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<Fetched> {
        let resp = self
            .client
            .post(format!("{BASE_URL}{path}"))
            .form(form)
            .send()
            .await
            .context("Connection error")?;
        collect(resp).await
    }
}

/// Reads a response and turns an expired session into `AuthFailed`.
///
/// A 404 is NOT treated as a retired endpoint here: `/machine/<unknown>`
/// legitimately answers 404, and `flag`/`writeups` classify that as
/// "machine not found". Stale-build detection lives in `login_with`, which
/// runs before any scraper and has no per-resource 404 semantics.
async fn collect(resp: reqwest::Response) -> Result<Fetched> {
    let status = resp.status().as_u16();
    let path = resp.url().path().to_string();
    let text = resp.text().await?;
    if path == LOGIN_PATH {
        return Err(crate::modules::HmvError::AuthFailed.into());
    }
    Ok(Fetched { status, path, text })
}

/// Logs in using stored credentials and returns the authenticated session.
pub async fn login(cfg: &ConfigManager) -> Result<HmvSession> {
    let (username, password) = cfg.load_credentials()?;
    login_with(&username, &password).await
}

/// Logs in with explicit credentials (used by the TUI first-run config popup,
/// where the credentials are not stored yet or no longer work).
pub async fn login_with(username: &str, password: &str) -> Result<HmvSession> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(15))
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("Failed to build HTTP client")?;

    let resp = client
        .post(format!("{BASE_URL}{LOGIN_PATH}"))
        .form(&[("username", username), ("password", password)])
        .send()
        .await
        .context("Connection error")?;

    let outcome = login_outcome(resp.status(), resp.headers());
    match outcome {
        LoginOutcome::Ok => Ok(HmvSession { client }),
        LoginOutcome::Gone => Err(crate::modules::HmvError::EndpointGone.into()),
        LoginOutcome::Credentials => Err(crate::modules::HmvError::AuthFailed.into()),
    }
}

/// What a `/login` response means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginOutcome {
    /// `Hx-Redirect` present: authenticated.
    Ok,
    /// 404: this build calls a route the platform no longer serves.
    Gone,
    /// Anything else without the success header: bad credentials.
    Credentials,
}

fn login_outcome(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> LoginOutcome {
    if status == reqwest::StatusCode::NOT_FOUND {
        return LoginOutcome::Gone;
    }
    if is_login_success(headers) {
        return LoginOutcome::Ok;
    }
    LoginOutcome::Credentials
}

/// The site answers a good login with `Hx-Redirect: /dashboard`; a bad one
/// with a `[-] Wrong Creds.` fragment and no such header.
fn is_login_success(headers: &reqwest::header::HeaderMap) -> bool {
    headers.contains_key("hx-redirect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_detects_missing_hx_redirect() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert!(!is_login_success(&headers));
        headers.insert("hx-redirect", "/dashboard".parse().unwrap());
        assert!(is_login_success(&headers));
    }

    #[test]
    fn login_outcome_prefers_gone_over_creds() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(
            login_outcome(reqwest::StatusCode::NOT_FOUND, &headers),
            LoginOutcome::Gone
        );
        assert_eq!(
            login_outcome(reqwest::StatusCode::OK, &headers),
            LoginOutcome::Credentials
        );
        headers.insert("hx-redirect", "/dashboard".parse().unwrap());
        assert_eq!(
            login_outcome(reqwest::StatusCode::OK, &headers),
            LoginOutcome::Ok
        );
    }
}
