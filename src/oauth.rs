//! Google sign-in: OAuth 2.0 for installed apps (authorization code + PKCE
//! S256), with the browser sent back to a one-shot listener on
//! `http://127.0.0.1:<port>/`.
//!
//! How the app signs in (never on the UI thread; [`Pending::wait`] blocks):
//!
//! ```no_run
//! # use std::sync::{Arc, atomic::AtomicBool};
//! # use std::time::Duration;
//! use calinator::oauth;
//! let Some(app) = oauth::google_app() else { return }; // None: "Google sign-in is not set up in this build."
//! let pending = oauth::begin(&app).unwrap();            // binds the listener, makes the URL
//! oauth::open_browser(pending.url());                   // also show the URL in case no browser opens
//! let cancel = Arc::new(AtomicBool::new(false));        // set it to stop waiting
//! let flag = cancel.clone();
//! let worker = std::thread::spawn(move || pending.wait(oauth::LOGIN_TIMEOUT, &flag));
//! let tokens = worker.join().unwrap().unwrap();         // Err: a short message for the user
//! // Save `tokens.refresh` with `accounts::set_secret(&account.id, ..)`;
//! // `tokens.email` is the account address (username).
//! ```
//!
//! Syncing then uses [`OAuthTokens`], which reads the refresh token from the
//! keyring and trades it for access tokens when they expire.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::accounts;
use crate::sync;

/// How long to wait for the browser by default.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Calendar events (read and write) and the calendar list (read only), plus
/// the account's email address.
const GOOGLE_SCOPES: [&str; 4] = [
    "https://www.googleapis.com/auth/calendar.events",
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly",
    "openid",
    "email",
];

/// One OAuth service, as registered for Calinator.
#[derive(Debug, Clone)]
pub struct OAuthApp {
    /// Shown to the user, e.g. "Google".
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub client_id: String,
    /// Installed apps can't keep a secret, but Google still wants it sent.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// Extra query parameters for the browser URL.
    pub extra_auth_params: Vec<(String, String)>,
}

/// Google Calendar. The client id and secret come from the environment
/// (`CALINATOR_GOOGLE_CLIENT_ID`, `CALINATOR_GOOGLE_CLIENT_SECRET`), else from
/// the same variables at build time. `None` when there is no client id.
pub fn google_app() -> Option<OAuthApp> {
    Some(OAuthApp {
        name: "Google",
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        client_id: setting("CALINATOR_GOOGLE_CLIENT_ID", option_env!("CALINATOR_GOOGLE_CLIENT_ID"))?,
        client_secret: setting("CALINATOR_GOOGLE_CLIENT_SECRET", option_env!("CALINATOR_GOOGLE_CLIENT_SECRET")),
        scopes: GOOGLE_SCOPES.iter().map(|s| s.to_string()).collect(),
        // `offline` + `consent`: Google sends a refresh token every time.
        extra_auth_params: vec![("access_type".into(), "offline".into()), ("prompt".into(), "consent".into())],
    })
}

/// A setting: the environment first, then the value built in at compile time.
fn setting(name: &str, built_in: Option<&'static str>) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty()).or_else(|| built_in.filter(|v| !v.is_empty()).map(str::to_string))
}

// ---------------------------------------------------------------- login

/// Tokens from a finished login. Save `refresh` with [`accounts::set_secret`].
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_at: Instant,
    /// The account's email, from the ID token.
    pub email: Option<String>,
}

/// A login waiting for the browser to come back.
pub struct Pending {
    app: OAuthApp,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    verifier: String,
    url: String,
}

/// Starts a login: listens on a free loopback port and builds the browser URL.
pub fn begin(app: &OAuthApp) -> Result<Pending, String> {
    let fail = |e: io::Error| format!("Could not start the login: {e}");
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(fail)?;
    listener.set_nonblocking(true).map_err(fail)?;
    let redirect_uri = format!("http://127.0.0.1:{}/", listener.local_addr().map_err(fail)?.port());
    let state = URL_SAFE_NO_PAD.encode(random_bytes::<16>().map_err(fail)?);
    let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<32>().map_err(fail)?);
    let url = auth_url(app, &redirect_uri, &state, &pkce_challenge(&verifier));
    Ok(Pending { app: app.clone(), listener, redirect_uri, state, verifier, url })
}

/// The whole login in one call: [`begin`], `open(url)`, then [`Pending::wait`].
/// Blocks: run it off the UI thread.
pub fn login(app: &OAuthApp, open: impl Fn(&str), timeout: Duration, cancel: &AtomicBool) -> Result<Tokens, String> {
    let pending = begin(app)?;
    open(pending.url());
    pending.wait(timeout, cancel)
}

impl Pending {
    /// The page to open in the browser.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Waits for the browser, then trades the code for tokens. Blocks: run it
    /// off the UI thread. Stops when `cancel` is set or after `timeout`.
    pub fn wait(self, timeout: Duration, cancel: &AtomicBool) -> Result<Tokens, String> {
        let code = self.receive(cancel, timeout)?;
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", self.app.client_id.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ];
        if let Some(secret) = &self.app.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let reply = post_form(self.app.token_url, &form).map_err(|e| format!("Login failed: {e}"))?;
        login_tokens(reply, Instant::now())
    }

    /// Accepts connections until the redirect comes, and returns its code.
    fn receive(&self, cancel: &AtomicBool, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Login canceled.".to_string());
            }
            if Instant::now() >= deadline {
                return Err("Login timed out. Try again.".to_string());
            }
            let mut stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted) => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(format!("Login failed: {e}")),
            };
            // Browsers also open spare connections and ask for /favicon.ico.
            let Some(target) = read_request_target(&mut stream) else { continue };
            let query = match target.split_once('?') {
                Some(("/", query)) if !query.is_empty() => query,
                _ => {
                    respond(&mut stream, "404 Not Found", "Not found.");
                    continue;
                }
            };
            let result = redirect_code(query, &self.state);
            let message = match &result {
                Ok(_) => "You can close this tab and go back to Calinator.",
                Err(e) => e.as_str(),
            };
            respond(&mut stream, "200 OK", message);
            return result;
        }
    }
}

/// The tokens of a code exchange. Without a refresh token the login is no use.
fn login_tokens(reply: TokenReply, now: Instant) -> Result<Tokens, String> {
    let expires_at = reply.expires_at(now);
    let Some(refresh) = reply.refresh_token.filter(|r| !r.is_empty()) else {
        return Err("Login failed: Google did not allow lasting access. Try again.".to_string());
    };
    Ok(Tokens { email: reply.id_token.as_deref().and_then(jwt_email), expires_at, access: reply.access_token, refresh })
}

/// The browser URL: the app's auth endpoint with every parameter encoded.
fn auth_url(app: &OAuthApp, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let scope = app.scopes.join(" ");
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", app.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("scope", scope.as_str()),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    params.extend(app.extra_auth_params.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let query: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect();
    format!("{}?{}", app.auth_url, query.join("&"))
}

/// The authorization code from the redirect query, after checking `state`.
fn redirect_code(query: &str, state: &str) -> Result<String, String> {
    let params = parse_query(query);
    if let Some(error) = params.get("error") {
        if error == "access_denied" {
            return Err("Login canceled in the browser.".to_string());
        }
        let detail = params.get("error_description").filter(|d| !d.is_empty()).unwrap_or(error);
        return Err(format!("Login failed: {detail}"));
    }
    if params.get("state").map(String::as_str) != Some(state) {
        return Err("Login failed: the reply did not match this login. Try again.".to_string());
    }
    match params.get("code") {
        Some(code) if !code.is_empty() => Ok(code.clone()),
        _ => Err("Login failed: no code in the reply.".to_string()),
    }
}

/// Reads an HTTP request head and returns the target of a GET (`/?code=…`).
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&head);
    let mut line = head.lines().next()?.split(' ');
    if line.next()? != "GET" {
        return None;
    }
    line.next().map(str::to_string)
}

/// Answers the browser with a tiny page.
fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Calinator</title>\
         <p style=\"font: 1.2em sans-serif; margin: 3em\">{}</p>",
        escape_html(message)
    );
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // The browser may be gone already; the login result does not depend on it.
    let _ = stream.write_all(reply.as_bytes()).and_then(|()| stream.flush());
}

/// Opens `url` in the default browser. Failures are silent: the UI also shows the URL.
pub fn open_browser(url: &str) {
    use std::process::{Command, Stdio};
    let child = Command::new("xdg-open").arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Ok(mut child) = child {
        // Reap it in the background so no zombie is left.
        std::thread::spawn(move || child.wait());
    }
}

// ---------------------------------------------------------------- tokens

/// Access tokens for one account. The refresh token lives in the keyring
/// under the account id.
pub struct OAuthTokens {
    app: OAuthApp,
    account_id: String,
    access: Option<(String, Instant)>,
}

/// Refresh this long before the access token expires.
const EARLY: Duration = Duration::from_secs(60);

impl OAuthTokens {
    /// `access`: a token and its expiry from a fresh login, so the first sync
    /// needs no refresh.
    pub fn new(app: OAuthApp, account_id: String, access: Option<(String, Instant)>) -> Self {
        OAuthTokens { app, account_id, access }
    }

    /// A valid access token, refreshed when it is about to expire.
    pub fn token(&mut self) -> sync::Result<String> {
        match &self.access {
            Some((token, expires_at)) if expires_at.saturating_duration_since(Instant::now()) > EARLY => Ok(token.clone()),
            _ => self.refresh(),
        }
    }

    /// A new access token (after a 401, or on expiry). A revoked login is
    /// [`sync::Error::Auth`].
    pub fn refresh(&mut self) -> sync::Result<String> {
        self.access = None;
        let old = accounts::get_secret(&self.account_id)
            .map_err(sync::Error::Auth)?
            .ok_or_else(|| sync::Error::Auth("No saved login. Sign in again.".to_string()))?;
        let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", old.as_str()), ("client_id", self.app.client_id.as_str())];
        if let Some(secret) = &self.app.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let reply = post_form(self.app.token_url, &form)?;
        let (access, rotated) = refreshed(&old, reply, Instant::now());
        if let Some(new) = rotated {
            accounts::set_secret(&self.account_id, &new).map_err(sync::Error::Auth)?;
        }
        let token = access.0.clone();
        self.access = Some(access);
        Ok(token)
    }
}

/// A token endpoint reply (RFC 6749 section 5.1).
#[derive(Debug, Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    id_token: Option<String>,
}

impl TokenReply {
    fn expires_at(&self, now: Instant) -> Instant {
        now + Duration::from_secs(self.expires_in.unwrap_or(3600))
    }
}

/// The new access token with its expiry, and the refresh token to save when
/// the server sent a new one.
fn refreshed(old_refresh: &str, reply: TokenReply, now: Instant) -> ((String, Instant), Option<String>) {
    let expires_at = reply.expires_at(now);
    let rotated = reply.refresh_token.filter(|r| !r.is_empty() && r != old_refresh);
    ((reply.access_token, expires_at), rotated)
}

/// POSTs a form to a token endpoint.
fn post_form(url: &str, form: &[(&str, &str)]) -> sync::Result<TokenReply> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build().into();
    let mut response = agent.post(url).header("Accept", "application/json").send_form(form.iter().copied()).map_err(sync::http_error)?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().map_err(sync::http_error)?;
    if status >= 400 {
        return Err(token_error(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| sync::Error::Parse(e.to_string()))
}

/// Maps a failed token request. `invalid_grant` means the login was revoked or expired.
fn token_error(status: u16, body: &str) -> sync::Error {
    let reply: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let code = reply["error"].as_str().unwrap_or_default();
    let message = match reply["error_description"].as_str().filter(|d| !d.is_empty()) {
        Some(description) => description.to_string(),
        None if !code.is_empty() => code.to_string(),
        None => format!("HTTP {status}"),
    };
    if status == 401 || code == "invalid_grant" { sync::Error::Auth(message) } else { sync::Error::Server(status, message) }
}

/// The email in an ID token. The token came straight from the token endpoint
/// over TLS, so the signature is not checked.
fn jwt_email(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims["email"].as_str().filter(|v| !v.is_empty()).map(str::to_string)
}

// ---------------------------------------------------------------- helpers

/// PKCE S256 challenge for a code verifier (RFC 7636 section 4.2).
fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Decodes a query value: `+` is a space, `%XX` a byte.
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => match bytes.get(i + 1..i + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit)) {
                Some(hex) => {
                    out.push(hex.iter().fold(0, |n, &h| n * 16 + (h as char).to_digit(16).unwrap_or(0) as u8));
                    i += 2;
                }
                None => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (url_decode(key), url_decode(value))
        })
        .collect()
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> OAuthApp {
        OAuthApp {
            name: "Google",
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            client_id: "123-abc.apps.googleusercontent.com".to_string(),
            client_secret: Some("GOCSPX-secret".to_string()),
            scopes: GOOGLE_SCOPES.iter().map(|s| s.to_string()).collect(),
            extra_auth_params: vec![("access_type".to_string(), "offline".to_string()), ("prompt".to_string(), "consent".to_string())],
        }
    }

    fn reply(json: &str) -> TokenReply {
        serde_json::from_str(json).unwrap()
    }

    fn id_token(claims: &str) -> String {
        format!("{}.{}.c2lnbmF0dXJl", URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#), URL_SAFE_NO_PAD.encode(claims))
    }

    #[test]
    fn pkce_matches_rfc_7636_appendix_b() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn auth_url_has_every_param_encoded() {
        let url = auth_url(&app(), "http://127.0.0.1:8080/", "st-1", "E9Mel_x-cM");
        assert_eq!(
            url,
            "https://accounts.google.com/o/oauth2/v2/auth?response_type=code\
             &client_id=123-abc.apps.googleusercontent.com\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2F\
             &scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.events\
             %20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.calendarlist.readonly%20openid%20email\
             &state=st-1&code_challenge=E9Mel_x-cM&code_challenge_method=S256\
             &access_type=offline&prompt=consent"
        );
    }

    #[test]
    fn begin_url_carries_state_and_challenge() {
        let pending = begin(&app()).unwrap();
        let params = parse_query(pending.url().split_once('?').unwrap().1);
        assert_eq!(params["state"], pending.state);
        assert_eq!(params["code_challenge"], pkce_challenge(&pending.verifier));
        assert_eq!(params["redirect_uri"], pending.redirect_uri);
        assert!(pending.redirect_uri.starts_with("http://127.0.0.1:"));
        assert_eq!(pending.verifier.len(), 43);
    }

    #[test]
    fn query_values_are_decoded() {
        let params = parse_query("code=4%2F0AeaYSHB_x+y%7e&state=a%3Db&flag&bad=%zz%4");
        assert_eq!(params["code"], "4/0AeaYSHB_x y~");
        assert_eq!(params["state"], "a=b");
        assert_eq!(params["flag"], "");
        assert_eq!(params["bad"], "%zz%4");
    }

    #[test]
    fn redirect_checks_state_and_reports_errors() {
        assert_eq!(redirect_code("state=s1&code=4%2F0Ab", "s1"), Ok("4/0Ab".to_string()));
        assert!(redirect_code("code=abc&state=other", "s1").unwrap_err().contains("did not match"));
        assert_eq!(redirect_code("error=access_denied&state=s1", "s1"), Err("Login canceled in the browser.".to_string()));
        assert_eq!(
            redirect_code("error=invalid_scope&error_description=Bad+scope&state=s1", "s1"),
            Err("Login failed: Bad scope".to_string())
        );
        assert_eq!(redirect_code("state=s1&code=", "s1"), Err("Login failed: no code in the reply.".to_string()));
    }

    #[test]
    fn login_needs_a_refresh_token_and_reads_the_email() {
        let now = Instant::now();
        let token = id_token(r#"{"iss":"https://accounts.google.com","sub":"1","email":"steve@example.com","email_verified":true}"#);
        let ok = reply(&format!(r#"{{"access_token":"ya29.a","refresh_token":"1//0g","expires_in":3599,"id_token":"{token}"}}"#));
        let t = login_tokens(ok, now).unwrap();
        assert_eq!((t.access.as_str(), t.refresh.as_str(), t.email.as_deref()), ("ya29.a", "1//0g", Some("steve@example.com")));
        assert_eq!(t.expires_at, now + Duration::from_secs(3599));
        assert!(login_tokens(reply(r#"{"access_token":"ya29.a"}"#), now).unwrap_err().contains("lasting access"));
        assert_eq!(jwt_email("not-a-jwt"), None);
    }

    #[test]
    fn only_a_new_refresh_token_is_saved() {
        let now = Instant::now();
        let ((access, _), rotated) = refreshed("1//old", reply(r#"{"access_token":"ya29.b","expires_in":3599}"#), now);
        assert_eq!((access.as_str(), rotated), ("ya29.b", None));
        let (_, rotated) = refreshed("1//old", reply(r#"{"access_token":"x","refresh_token":"1//new"}"#), now);
        assert_eq!(rotated, Some("1//new".to_string()));
    }

    #[test]
    fn revoked_login_is_an_auth_error() {
        let revoked = r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        assert_eq!(token_error(400, revoked), sync::Error::Auth("Token has been expired or revoked.".to_string()));
        assert_eq!(token_error(400, r#"{"error":"invalid_request"}"#), sync::Error::Server(400, "invalid_request".to_string()));
        assert_eq!(token_error(503, "<html>down</html>"), sync::Error::Server(503, "HTTP 503".to_string()));
    }

    /// Sends one request to the login listener like a browser and returns the reply.
    fn browse(addr: std::net::SocketAddr, target: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: {addr}\r\n\r\n").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn wait_rejects_wrong_state_and_stops_on_cancel() {
        let pending = begin(&app()).unwrap();
        let addr = pending.listener.local_addr().unwrap();
        let browser = std::thread::spawn(move || (browse(addr, "/favicon.ico"), browse(addr, "/?code=abc&state=wrong")));
        let result = pending.wait(Duration::from_secs(10), &AtomicBool::new(false));
        let (favicon, redirect) = browser.join().unwrap();
        assert!(result.unwrap_err().contains("did not match"));
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(redirect.starts_with("HTTP/1.1 200") && redirect.contains("did not match"));

        let pending = begin(&app()).unwrap();
        assert_eq!(pending.wait(Duration::from_secs(10), &AtomicBool::new(true)).unwrap_err(), "Login canceled.");
        let pending = begin(&app()).unwrap();
        assert_eq!(pending.wait(Duration::ZERO, &AtomicBool::new(false)).unwrap_err(), "Login timed out. Try again.");
    }
}
