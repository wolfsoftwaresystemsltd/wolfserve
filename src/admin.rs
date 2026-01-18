//! Admin Dashboard Module for WolfServe
//! Provides authentication, statistics, and monitoring on port 5000

use axum::{
    extract::{State, Form},
    http::{StatusCode, HeaderMap, header},
    response::{Response, IntoResponse, Html, Redirect},
    routing::get,
    Router,
    body::Body,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::fs;
use std::collections::VecDeque;
use parking_lot::RwLock;
use chrono::{DateTime, Utc, Duration};
use uuid::Uuid;

const CREDENTIALS_FILE: &str = "wolfserve_admin.dat";
const MAX_LOG_ENTRIES: usize = 50;
const SESSION_TIMEOUT_HOURS: i64 = 24;

/// Request log entry
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RequestLogEntry {
    pub timestamp: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub client_ip: String,
    pub host: String,
    pub user_agent: String,
}

/// Server statistics
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct ServerStats {
    pub total_requests: u64,
    pub requests_2xx: u64,
    pub requests_3xx: u64,
    pub requests_4xx: u64,
    pub requests_5xx: u64,
    pub total_response_time_ms: u64,
    pub start_time: Option<DateTime<Utc>>,
    pub bytes_sent: u64,
}

impl ServerStats {
    pub fn avg_response_time_ms(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_response_time_ms as f64 / self.total_requests as f64
        }
    }
    
    pub fn requests_per_second(&self) -> f64 {
        if let Some(start) = self.start_time {
            let elapsed = Utc::now().signed_duration_since(start);
            let secs = elapsed.num_seconds().max(1) as f64;
            self.total_requests as f64 / secs
        } else {
            0.0
        }
    }
    
    pub fn uptime_string(&self) -> String {
        if let Some(start) = self.start_time {
            let elapsed = Utc::now().signed_duration_since(start);
            let days = elapsed.num_days();
            let hours = elapsed.num_hours() % 24;
            let minutes = elapsed.num_minutes() % 60;
            let seconds = elapsed.num_seconds() % 60;
            format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
        } else {
            "Unknown".to_string()
        }
    }
}

/// Session for authenticated users
#[derive(Clone, Debug)]
struct Session {
    token: String,
    created_at: DateTime<Utc>,
    username: String,
}

/// Stored credentials (encrypted)
#[derive(Serialize, Deserialize)]
struct StoredCredentials {
    username: String,
    password_hash: String,
}

/// Admin state
pub struct AdminState {
    pub logs: RwLock<VecDeque<RequestLogEntry>>,
    pub stats: RwLock<ServerStats>,
    sessions: RwLock<Vec<Session>>,
}

impl AdminState {
    pub fn new() -> Self {
        let mut stats = ServerStats::default();
        stats.start_time = Some(Utc::now());
        
        Self {
            logs: RwLock::new(VecDeque::with_capacity(MAX_LOG_ENTRIES)),
            stats: RwLock::new(stats),
            sessions: RwLock::new(Vec::new()),
        }
    }
    
    /// Log a request
    pub fn log_request(&self, entry: RequestLogEntry) {
        // Update stats
        {
            let mut stats = self.stats.write();
            stats.total_requests += 1;
            stats.total_response_time_ms += entry.duration_ms;
            
            match entry.status {
                200..=299 => stats.requests_2xx += 1,
                300..=399 => stats.requests_3xx += 1,
                400..=499 => stats.requests_4xx += 1,
                500..=599 => stats.requests_5xx += 1,
                _ => {}
            }
        }
        
        // Add log entry
        {
            let mut logs = self.logs.write();
            if logs.len() >= MAX_LOG_ENTRIES {
                logs.pop_front();
            }
            logs.push_back(entry);
        }
    }
    
    /// Create a new session
    fn create_session(&self, username: &str) -> String {
        let token = Uuid::new_v4().to_string();
        let session = Session {
            token: token.clone(),
            created_at: Utc::now(),
            username: username.to_string(),
        };
        
        // Clean up expired sessions and add new one
        let mut sessions = self.sessions.write();
        let cutoff = Utc::now() - Duration::hours(SESSION_TIMEOUT_HOURS);
        sessions.retain(|s| s.created_at > cutoff);
        sessions.push(session);
        
        token
    }
    
    /// Validate a session token
    fn validate_session(&self, token: &str) -> Option<String> {
        let sessions = self.sessions.read();
        let cutoff = Utc::now() - Duration::hours(SESSION_TIMEOUT_HOURS);
        
        sessions.iter()
            .find(|s| s.token == token && s.created_at > cutoff)
            .map(|s| s.username.clone())
    }
    
    /// Remove a session
    fn remove_session(&self, token: &str) {
        let mut sessions = self.sessions.write();
        sessions.retain(|s| s.token != token);
    }
}

/// Load or create default credentials
fn load_credentials() -> StoredCredentials {
    if let Ok(data) = fs::read_to_string(CREDENTIALS_FILE) {
        // Decode from base64
        if let Ok(decoded) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data) {
            if let Ok(json) = String::from_utf8(decoded) {
                if let Ok(creds) = serde_json::from_str::<StoredCredentials>(&json) {
                    return creds;
                }
            }
        }
    }
    
    // Create default credentials
    let default_hash = bcrypt::hash("admin", bcrypt::DEFAULT_COST).unwrap();
    let creds = StoredCredentials {
        username: "admin".to_string(),
        password_hash: default_hash,
    };
    
    save_credentials(&creds);
    creds
}

/// Save credentials to encrypted file
fn save_credentials(creds: &StoredCredentials) {
    let json = serde_json::to_string(creds).unwrap();
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, json.as_bytes());
    let _ = fs::write(CREDENTIALS_FILE, encoded);
}

/// Get session token from cookie
fn get_session_token(headers: &HeaderMap) -> Option<String> {
    headers.get(header::COOKIE)?
        .to_str().ok()?
        .split(';')
        .find_map(|cookie| {
            let parts: Vec<&str> = cookie.trim().splitn(2, '=').collect();
            if parts.len() == 2 && parts[0] == "wolfserve_session" {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
}

/// Check if request is authenticated
fn is_authenticated(headers: &HeaderMap, state: &AdminState) -> Option<String> {
    let token = get_session_token(headers)?;
    state.validate_session(&token)
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    current_password: String,
    new_password: String,
    confirm_password: String,
}

/// Create the admin router
pub fn admin_router(state: Arc<AdminState>) -> Router {
    Router::new()
        .route("/", get(dashboard_handler))
        .route("/login", get(login_page).post(login_handler))
        .route("/logout", get(logout_handler))
        .route("/change-password", get(change_password_page).post(change_password_handler))
        .route("/api/stats", get(api_stats))
        .route("/api/logs", get(api_logs))
        .with_state(state)
}

async fn login_page() -> Html<String> {
    Html(LOGIN_HTML.to_string())
}

async fn login_handler(
    State(state): State<Arc<AdminState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    let creds = load_credentials();
    
    if form.username == creds.username {
        if let Ok(true) = bcrypt::verify(&form.password, &creds.password_hash) {
            let token = state.create_session(&form.username);
            
            return Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::LOCATION, "/")
                .header(
                    header::SET_COOKIE,
                    format!("wolfserve_session={}; Path=/; HttpOnly; SameSite=Strict", token)
                )
                .body(Body::empty())
                .unwrap();
        }
    }
    
    Html(LOGIN_HTML.replace("<!-- ERROR -->", 
        r#"<div class="error">Invalid username or password</div>"#)).into_response()
}

async fn logout_handler(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(token) = get_session_token(&headers) {
        state.remove_session(&token);
    }
    
    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, "/login")
        .header(
            header::SET_COOKIE,
            "wolfserve_session=; Path=/; HttpOnly; Max-Age=0"
        )
        .body(Body::empty())
        .unwrap()
}

async fn dashboard_handler(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    match is_authenticated(&headers, &state) {
        Some(username) => {
            let stats = state.stats.read().clone();
            let logs = state.logs.read().clone();
            
            let html = generate_dashboard_html(&username, &stats, &logs);
            Html(html).into_response()
        }
        None => {
            Redirect::to("/login").into_response()
        }
    }
}

async fn change_password_page(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    match is_authenticated(&headers, &state) {
        Some(_) => Html(CHANGE_PASSWORD_HTML.to_string()).into_response(),
        None => Redirect::to("/login").into_response(),
    }
}

async fn change_password_handler(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    Form(form): Form<ChangePasswordForm>,
) -> Response {
    if is_authenticated(&headers, &state).is_none() {
        return Redirect::to("/login").into_response();
    }
    
    let creds = load_credentials();
    
    // Verify current password
    if bcrypt::verify(&form.current_password, &creds.password_hash).unwrap_or(false) {
        if form.new_password == form.confirm_password {
            if form.new_password.len() >= 4 {
                let new_hash = bcrypt::hash(&form.new_password, bcrypt::DEFAULT_COST).unwrap();
                let new_creds = StoredCredentials {
                    username: creds.username,
                    password_hash: new_hash,
                };
                save_credentials(&new_creds);
                
                return Html(CHANGE_PASSWORD_HTML.replace("<!-- MESSAGE -->",
                    r#"<div class="success">Password changed successfully!</div>"#)).into_response();
            } else {
                return Html(CHANGE_PASSWORD_HTML.replace("<!-- MESSAGE -->",
                    r#"<div class="error">Password must be at least 4 characters</div>"#)).into_response();
            }
        } else {
            return Html(CHANGE_PASSWORD_HTML.replace("<!-- MESSAGE -->",
                r#"<div class="error">New passwords do not match</div>"#)).into_response();
        }
    }
    
    Html(CHANGE_PASSWORD_HTML.replace("<!-- MESSAGE -->",
        r#"<div class="error">Current password is incorrect</div>"#)).into_response()
}

async fn api_stats(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    if is_authenticated(&headers, &state).is_none() {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    
    let stats = state.stats.read();
    let json = serde_json::json!({
        "total_requests": stats.total_requests,
        "requests_2xx": stats.requests_2xx,
        "requests_3xx": stats.requests_3xx,
        "requests_4xx": stats.requests_4xx,
        "requests_5xx": stats.requests_5xx,
        "avg_response_time_ms": stats.avg_response_time_ms(),
        "requests_per_second": stats.requests_per_second(),
        "uptime": stats.uptime_string(),
    });
    
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

async fn api_logs(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
) -> Response {
    if is_authenticated(&headers, &state).is_none() {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    
    let logs: Vec<_> = state.logs.read().iter().rev().cloned().collect();
    let json = serde_json::to_string(&logs).unwrap();
    
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json))
        .unwrap()
}

fn generate_dashboard_html(username: &str, stats: &ServerStats, logs: &VecDeque<RequestLogEntry>) -> String {
    let logs_html: String = logs.iter().rev().map(|log| {
        let status_class = match log.status {
            200..=299 => "status-2xx",
            300..=399 => "status-3xx",
            400..=499 => "status-4xx",
            _ => "status-5xx",
        };
        format!(
            r#"<tr>
                <td>{}</td>
                <td><span class="method {}">{}</span></td>
                <td class="path">{}</td>
                <td><span class="status {}">{}</span></td>
                <td>{}ms</td>
                <td>{}</td>
                <td>{}</td>
            </tr>"#,
            log.timestamp.format("%Y-%m-%d %H:%M:%S"),
            log.method.to_lowercase(),
            log.method,
            log.path,
            status_class,
            log.status,
            log.duration_ms,
            log.client_ip,
            log.host,
        )
    }).collect();
    
    DASHBOARD_HTML
        .replace("{{USERNAME}}", username)
        .replace("{{UPTIME}}", &stats.uptime_string())
        .replace("{{TOTAL_REQUESTS}}", &stats.total_requests.to_string())
        .replace("{{REQUESTS_2XX}}", &stats.requests_2xx.to_string())
        .replace("{{REQUESTS_3XX}}", &stats.requests_3xx.to_string())
        .replace("{{REQUESTS_4XX}}", &stats.requests_4xx.to_string())
        .replace("{{REQUESTS_5XX}}", &stats.requests_5xx.to_string())
        .replace("{{AVG_RESPONSE_TIME}}", &format!("{:.2}", stats.avg_response_time_ms()))
        .replace("{{REQUESTS_PER_SEC}}", &format!("{:.2}", stats.requests_per_second()))
        .replace("{{LOGS_TABLE}}", &logs_html)
}

const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WolfServe Admin - Login</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .login-container {
            background: rgba(255,255,255,0.1);
            backdrop-filter: blur(10px);
            padding: 40px;
            border-radius: 16px;
            box-shadow: 0 8px 32px rgba(0,0,0,0.3);
            width: 100%;
            max-width: 400px;
        }
        .logo {
            text-align: center;
            margin-bottom: 30px;
            color: #fff;
        }
        .logo h1 { font-size: 28px; margin-bottom: 5px; }
        .logo p { color: #888; font-size: 14px; }
        .form-group { margin-bottom: 20px; }
        label {
            display: block;
            color: #ccc;
            margin-bottom: 8px;
            font-size: 14px;
        }
        input[type="text"], input[type="password"] {
            width: 100%;
            padding: 12px 16px;
            border: 1px solid rgba(255,255,255,0.2);
            border-radius: 8px;
            background: rgba(255,255,255,0.1);
            color: #fff;
            font-size: 16px;
            transition: border-color 0.3s;
        }
        input:focus {
            outline: none;
            border-color: #4facfe;
        }
        button {
            width: 100%;
            padding: 14px;
            background: linear-gradient(135deg, #4facfe 0%, #00f2fe 100%);
            border: none;
            border-radius: 8px;
            color: #fff;
            font-size: 16px;
            font-weight: 600;
            cursor: pointer;
            transition: transform 0.2s, box-shadow 0.2s;
        }
        button:hover {
            transform: translateY(-2px);
            box-shadow: 0 4px 20px rgba(79,172,254,0.4);
        }
        .error {
            background: rgba(255,82,82,0.2);
            border: 1px solid #ff5252;
            color: #ff5252;
            padding: 12px;
            border-radius: 8px;
            margin-bottom: 20px;
            text-align: center;
        }
    </style>
</head>
<body>
    <div class="login-container">
        <div class="logo">
            <h1>🐺 WolfServe</h1>
            <p>Admin Dashboard</p>
        </div>
        <!-- ERROR -->
        <form method="POST" action="/login">
            <div class="form-group">
                <label for="username">Username</label>
                <input type="text" id="username" name="username" required autocomplete="username">
            </div>
            <div class="form-group">
                <label for="password">Password</label>
                <input type="password" id="password" name="password" required autocomplete="current-password">
            </div>
            <button type="submit">Sign In</button>
        </form>
    </div>
</body>
</html>"#;

const CHANGE_PASSWORD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WolfServe Admin - Change Password</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .container {
            background: rgba(255,255,255,0.1);
            backdrop-filter: blur(10px);
            padding: 40px;
            border-radius: 16px;
            box-shadow: 0 8px 32px rgba(0,0,0,0.3);
            width: 100%;
            max-width: 450px;
        }
        h1 {
            color: #fff;
            text-align: center;
            margin-bottom: 30px;
        }
        .form-group { margin-bottom: 20px; }
        label {
            display: block;
            color: #ccc;
            margin-bottom: 8px;
            font-size: 14px;
        }
        input[type="password"] {
            width: 100%;
            padding: 12px 16px;
            border: 1px solid rgba(255,255,255,0.2);
            border-radius: 8px;
            background: rgba(255,255,255,0.1);
            color: #fff;
            font-size: 16px;
        }
        input:focus { outline: none; border-color: #4facfe; }
        button {
            width: 100%;
            padding: 14px;
            background: linear-gradient(135deg, #4facfe 0%, #00f2fe 100%);
            border: none;
            border-radius: 8px;
            color: #fff;
            font-size: 16px;
            font-weight: 600;
            cursor: pointer;
            margin-bottom: 15px;
        }
        button:hover { transform: translateY(-2px); }
        .back-link {
            display: block;
            text-align: center;
            color: #4facfe;
            text-decoration: none;
        }
        .error {
            background: rgba(255,82,82,0.2);
            border: 1px solid #ff5252;
            color: #ff5252;
            padding: 12px;
            border-radius: 8px;
            margin-bottom: 20px;
            text-align: center;
        }
        .success {
            background: rgba(76,175,80,0.2);
            border: 1px solid #4caf50;
            color: #4caf50;
            padding: 12px;
            border-radius: 8px;
            margin-bottom: 20px;
            text-align: center;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>🔐 Change Password</h1>
        <!-- MESSAGE -->
        <form method="POST" action="/change-password">
            <div class="form-group">
                <label for="current_password">Current Password</label>
                <input type="password" id="current_password" name="current_password" required>
            </div>
            <div class="form-group">
                <label for="new_password">New Password</label>
                <input type="password" id="new_password" name="new_password" required minlength="4">
            </div>
            <div class="form-group">
                <label for="confirm_password">Confirm New Password</label>
                <input type="password" id="confirm_password" name="confirm_password" required minlength="4">
            </div>
            <button type="submit">Change Password</button>
        </form>
        <a href="/" class="back-link">← Back to Dashboard</a>
    </div>
</body>
</html>"#;

const DASHBOARD_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WolfServe Admin Dashboard</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: #0f0f1a;
            color: #fff;
            min-height: 100vh;
        }
        .header {
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            padding: 20px 30px;
            display: flex;
            justify-content: space-between;
            align-items: center;
            border-bottom: 1px solid rgba(255,255,255,0.1);
        }
        .logo {
            display: flex;
            align-items: center;
            gap: 15px;
        }
        .logo h1 { font-size: 24px; }
        .logo span { color: #4facfe; }
        .user-info {
            display: flex;
            align-items: center;
            gap: 20px;
        }
        .user-info a {
            color: #888;
            text-decoration: none;
            padding: 8px 16px;
            border-radius: 6px;
            transition: all 0.3s;
        }
        .user-info a:hover { background: rgba(255,255,255,0.1); color: #fff; }
        .user-info .logout { color: #ff5252; }
        .container { padding: 30px; max-width: 1600px; margin: 0 auto; }
        .stats-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 20px;
            margin-bottom: 30px;
        }
        .stat-card {
            background: linear-gradient(135deg, rgba(255,255,255,0.1) 0%, rgba(255,255,255,0.05) 100%);
            padding: 25px;
            border-radius: 12px;
            border: 1px solid rgba(255,255,255,0.1);
        }
        .stat-card h3 {
            color: #888;
            font-size: 12px;
            text-transform: uppercase;
            letter-spacing: 1px;
            margin-bottom: 10px;
        }
        .stat-card .value {
            font-size: 32px;
            font-weight: 700;
            background: linear-gradient(135deg, #4facfe 0%, #00f2fe 100%);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
            background-clip: text;
        }
        .stat-card.success .value { background: linear-gradient(135deg, #4caf50 0%, #8bc34a 100%); -webkit-background-clip: text; background-clip: text; }
        .stat-card.warning .value { background: linear-gradient(135deg, #ff9800 0%, #ffc107 100%); -webkit-background-clip: text; background-clip: text; }
        .stat-card.error .value { background: linear-gradient(135deg, #f44336 0%, #ff5252 100%); -webkit-background-clip: text; background-clip: text; }
        
        .logs-section {
            background: rgba(255,255,255,0.05);
            border-radius: 12px;
            border: 1px solid rgba(255,255,255,0.1);
            overflow: hidden;
        }
        .logs-header {
            padding: 20px;
            border-bottom: 1px solid rgba(255,255,255,0.1);
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .logs-header h2 { font-size: 18px; }
        .refresh-btn {
            background: rgba(79,172,254,0.2);
            color: #4facfe;
            border: 1px solid #4facfe;
            padding: 8px 16px;
            border-radius: 6px;
            cursor: pointer;
            font-size: 14px;
            transition: all 0.3s;
        }
        .refresh-btn:hover { background: #4facfe; color: #fff; }
        
        table {
            width: 100%;
            border-collapse: collapse;
        }
        th, td {
            padding: 14px 16px;
            text-align: left;
            border-bottom: 1px solid rgba(255,255,255,0.05);
        }
        th {
            background: rgba(0,0,0,0.2);
            font-size: 12px;
            text-transform: uppercase;
            letter-spacing: 1px;
            color: #888;
        }
        tr:hover { background: rgba(255,255,255,0.03); }
        
        .method {
            display: inline-block;
            padding: 4px 10px;
            border-radius: 4px;
            font-size: 12px;
            font-weight: 600;
        }
        .method.get { background: rgba(76,175,80,0.2); color: #4caf50; }
        .method.post { background: rgba(33,150,243,0.2); color: #2196f3; }
        .method.put { background: rgba(255,152,0,0.2); color: #ff9800; }
        .method.delete { background: rgba(244,67,54,0.2); color: #f44336; }
        
        .status {
            display: inline-block;
            padding: 4px 10px;
            border-radius: 4px;
            font-size: 12px;
            font-weight: 600;
        }
        .status-2xx { background: rgba(76,175,80,0.2); color: #4caf50; }
        .status-3xx { background: rgba(33,150,243,0.2); color: #2196f3; }
        .status-4xx { background: rgba(255,152,0,0.2); color: #ff9800; }
        .status-5xx { background: rgba(244,67,54,0.2); color: #f44336; }
        
        .path {
            max-width: 300px;
            overflow: hidden;
            text-overflow: ellipsis;
            white-space: nowrap;
            font-family: 'Monaco', 'Menlo', monospace;
            font-size: 13px;
        }
        
        .empty-state {
            padding: 60px 20px;
            text-align: center;
            color: #666;
        }
        
        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }
        .live-indicator {
            display: inline-block;
            width: 8px;
            height: 8px;
            background: #4caf50;
            border-radius: 50%;
            margin-right: 8px;
            animation: pulse 2s infinite;
        }
    </style>
</head>
<body>
    <div class="header">
        <div class="logo">
            <h1>🐺 WolfServe</h1>
            <span>Admin Dashboard</span>
        </div>
        <div class="user-info">
            <span>👤 {{USERNAME}}</span>
            <a href="/change-password">Change Password</a>
            <a href="/logout" class="logout">Logout</a>
        </div>
    </div>
    
    <div class="container">
        <div class="stats-grid">
            <div class="stat-card">
                <h3>Uptime</h3>
                <div class="value" id="uptime">{{UPTIME}}</div>
            </div>
            <div class="stat-card">
                <h3>Total Requests</h3>
                <div class="value" id="total-requests">{{TOTAL_REQUESTS}}</div>
            </div>
            <div class="stat-card success">
                <h3>2xx Success</h3>
                <div class="value" id="requests-2xx">{{REQUESTS_2XX}}</div>
            </div>
            <div class="stat-card">
                <h3>3xx Redirect</h3>
                <div class="value" id="requests-3xx">{{REQUESTS_3XX}}</div>
            </div>
            <div class="stat-card warning">
                <h3>4xx Client Error</h3>
                <div class="value" id="requests-4xx">{{REQUESTS_4XX}}</div>
            </div>
            <div class="stat-card error">
                <h3>5xx Server Error</h3>
                <div class="value" id="requests-5xx">{{REQUESTS_5XX}}</div>
            </div>
            <div class="stat-card">
                <h3>Avg Response Time</h3>
                <div class="value" id="avg-response">{{AVG_RESPONSE_TIME}}ms</div>
            </div>
            <div class="stat-card">
                <h3>Requests/sec</h3>
                <div class="value" id="req-per-sec">{{REQUESTS_PER_SEC}}</div>
            </div>
        </div>
        
        <div class="logs-section">
            <div class="logs-header">
                <h2><span class="live-indicator"></span>Recent Requests (Last 50)</h2>
                <button class="refresh-btn" onclick="refreshData()">↻ Refresh</button>
            </div>
            <table>
                <thead>
                    <tr>
                        <th>Time</th>
                        <th>Method</th>
                        <th>Path</th>
                        <th>Status</th>
                        <th>Duration</th>
                        <th>Client IP</th>
                        <th>Host</th>
                    </tr>
                </thead>
                <tbody id="logs-table">
                    {{LOGS_TABLE}}
                </tbody>
            </table>
            <div class="empty-state" id="empty-state" style="display: none;">
                No requests logged yet. Start making requests to see them here.
            </div>
        </div>
    </div>
    
    <script>
        function refreshData() {
            fetch('/api/stats')
                .then(r => r.json())
                .then(data => {
                    document.getElementById('uptime').textContent = data.uptime;
                    document.getElementById('total-requests').textContent = data.total_requests;
                    document.getElementById('requests-2xx').textContent = data.requests_2xx;
                    document.getElementById('requests-3xx').textContent = data.requests_3xx;
                    document.getElementById('requests-4xx').textContent = data.requests_4xx;
                    document.getElementById('requests-5xx').textContent = data.requests_5xx;
                    document.getElementById('avg-response').textContent = data.avg_response_time_ms.toFixed(2) + 'ms';
                    document.getElementById('req-per-sec').textContent = data.requests_per_second.toFixed(2);
                });
            
            fetch('/api/logs')
                .then(r => r.json())
                .then(logs => {
                    const tbody = document.getElementById('logs-table');
                    const empty = document.getElementById('empty-state');
                    
                    if (logs.length === 0) {
                        tbody.innerHTML = '';
                        empty.style.display = 'block';
                        return;
                    }
                    
                    empty.style.display = 'none';
                    tbody.innerHTML = logs.map(log => {
                        const statusClass = log.status >= 500 ? 'status-5xx' : 
                                           log.status >= 400 ? 'status-4xx' :
                                           log.status >= 300 ? 'status-3xx' : 'status-2xx';
                        return `<tr>
                            <td>${new Date(log.timestamp).toLocaleString()}</td>
                            <td><span class="method ${log.method.toLowerCase()}">${log.method}</span></td>
                            <td class="path">${log.path}</td>
                            <td><span class="status ${statusClass}">${log.status}</span></td>
                            <td>${log.duration_ms}ms</td>
                            <td>${log.client_ip}</td>
                            <td>${log.host}</td>
                        </tr>`;
                    }).join('');
                });
        }
        
        // Auto-refresh every 5 seconds
        setInterval(refreshData, 5000);
    </script>
</body>
</html>"##;
