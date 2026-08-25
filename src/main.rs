use axum::{
    extract::{Request, State},
    http::{StatusCode, HeaderMap},
    response::{Response, IntoResponse},
    routing::any,
    Router,
};
use std::path::{Path, PathBuf};
use tokio::fs;
use fastcgi_client::{Client, Params, Request as FcgiRequest};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::{timeout, Duration, Instant};
use http_body_util::BodyExt;
use std::borrow::Cow;
use serde::Deserialize;
use std::sync::Arc;
use std::collections::HashMap;
use std::net::SocketAddr;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::fs::File;
use std::io::BufReader;
use tokio_rustls::TlsAcceptor;
use futures_util::future::join_all;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tower_http::compression::CompressionLayer;
use chrono::Utc;
use percent_encoding::percent_decode_str;

mod apache;
mod admin;
use apache::{VirtualHost, RewriteContext, RewriteResult};
use admin::{AdminState, RequestLogEntry, admin_router};
use hyper_util::rt::TokioIo;

#[derive(Clone)]
pub struct TowerToHyperService<S> {
    service: S,
}

impl<S, R> hyper::service::Service<R> for TowerToHyperService<S>
where
    S: tower::Service<R> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, req: R) -> Self::Future {
        self.service.clone().call(req)
    }
}

#[derive(Debug)]


struct ServerCertResolver {
    certs: HashMap<String, Arc<CertifiedKey>>,
    default_cert: Option<Arc<CertifiedKey>>,
}

impl ResolvesServerCert for ServerCertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if let Some(sni_hostname) = client_hello.server_name() {
             if let Some(cert) = self.certs.get(sni_hostname) {
                 return Some(cert.clone());
             }
        }
        self.default_cert.clone()
    }
}

fn load_ssl_keys(cert_path: &Path, key_path: &Path, chain_path: Option<&PathBuf>) -> anyhow::Result<CertifiedKey> {
    let cert_file = &mut BufReader::new(File::open(cert_path)?);
    let key_file = &mut BufReader::new(File::open(key_path)?);

    let mut cert_chain = rustls_pemfile::certs(cert_file)
        .collect::<Result<Vec<_>, _>>()?;
    
    if let Some(cp) = chain_path {
        let chain_file = &mut BufReader::new(File::open(cp)?);
        let extra_certs = rustls_pemfile::certs(chain_file)
            .collect::<Result<Vec<_>, _>>()?;
        cert_chain.extend(extra_certs);
    }
    
    let mut keys = Vec::new();
    for item in rustls_pemfile::read_all(key_file) {
        match item? {
            rustls_pemfile::Item::Pkcs1Key(key) => keys.push(key.into()),
            rustls_pemfile::Item::Pkcs8Key(key) => keys.push(key.into()),
            rustls_pemfile::Item::Sec1Key(key) => keys.push(key.into()),
            _ => {},
        }
    }
        
    if keys.is_empty() {
        anyhow::bail!("No private keys found in {}", key_path.display());
    }
    
    let key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&keys[0])
        .map_err(|_| anyhow::anyhow!("Invalid private key"))?;
        
    Ok(CertifiedKey::new(cert_chain, key))
}



#[derive(Deserialize, Clone, Debug)]
struct Config {
    server: ServerConfig,
    php: PhpConfig,
    #[serde(default)]
    apache: ApacheConfig,
}

fn default_apache_dir() -> String {
    "/etc/apache2".to_string()
}

#[derive(Deserialize, Clone, Debug)]
struct ApacheConfig {
    #[serde(default = "default_apache_dir")]
    config_dir: String,
}

impl Default for ApacheConfig {
    fn default() -> Self {
        Self {
            config_dir: default_apache_dir(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
struct ServerConfig {
    host: String,
    port: u16,
}

#[derive(Deserialize, Clone, Debug)]
struct PhpConfig {
    fpm_address: Option<String>,
    #[serde(default = "default_php_mode")]
    mode: String, // "fpm" or "cgi"
    #[serde(default = "default_cgi_path")]
    cgi_path: String,
    /// PHP session save path (e.g., "/mnt/shared/wolfserve/sessions")
    /// Used by shell scripts for PHP-FPM configuration
    #[allow(dead_code)]
    session_save_path: Option<String>,
}

fn default_php_mode() -> String {
    "fpm".to_string()
}

fn default_cgi_path() -> String {
    "php-cgi".to_string()
}

struct AppState {
    config: Config,
    vhosts: HashMap<String, VirtualHost>, // Map Host header -> VirtualHost
    default_vhost: Option<VirtualHost>,
    admin_state: Arc<AdminState>,
}

fn is_common_connection_error(err: &dyn std::error::Error) -> bool {
    let s = format!("{:?}", err);
    s.contains("BrokenPipe") || 
    s.contains("ConnectionReset") || 
    s.contains("UnexpectedEof") ||
    s.contains("ConnectionAborted") ||
    s.contains("NotConnected") ||
    s.contains("TimedOut") ||
    s.contains("IncompleteMessage")
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    // CLI args FIRST — `wolfserve --version` / `--test` must never start a
    // server or write a default config. (The wolfproxy v0.4.7 lesson:
    // WolfStack's component probes ran the full server and orphaned a
    // listener holding the ports.)
    let args: Vec<String> = std::env::args().collect();
    let mut config_path = String::from("wolfserve.toml");
    let mut test_only = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--version" | "-V" => {
                println!("wolfserve {}", VERSION);
                return;
            }
            "--help" | "-h" => {
                println!(
                    "wolfserve {}\n\nUsage: wolfserve [OPTIONS]\n\nOptions:\n  -c, --config <path>  Config file (default: ./wolfserve.toml)\n  -t, --test           Validate config + Apache vhosts, then exit\n  -V, --version        Print version\n  -h, --help           Show this help",
                    VERSION
                );
                return;
            }
            "--test" | "-t" => test_only = true,
            "--config" | "-c" => {
                if i + 1 >= args.len() {
                    eprintln!("wolfserve: --config needs a path");
                    std::process::exit(2);
                }
                config_path = args[i + 1].clone();
                i += 1;
            }
            other => {
                eprintln!("wolfserve: unknown argument '{}' (try --help)", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if test_only {
        // Validate-and-exit: read the config WITHOUT creating a default
        // (a probe must not litter the cwd), parse it, and walk the
        // Apache vhost dir the way startup would.
        let config_str = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("wolfserve: cannot read {}: {}", config_path, e);
                std::process::exit(1);
            }
        };
        let config: Config = match toml::from_str(&config_str) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("wolfserve: config parse error in {}: {}", config_path, e);
                std::process::exit(1);
            }
        };
        let vhosts = apache::load_apache_config(Path::new(&config.apache.config_dir));
        println!(
            "wolfserve: configuration OK — {} vhost(s) from {}",
            vhosts.len(),
            config.apache.config_dir
        );
        return;
    }

    println!(r#"
 __          ______  _      ______  _____  ______  _____ __      __ ______ 
 \ \        / / __ \| |    |  ____|/ ____||  ____||  __ \\ \    / /|  ____|
  \ \  /\  / / |  | | |    | |__  | (___  | |__   | |__) |\ \  / / | |__   
   \ \/  \/ /| |  | | |    |  __|  \___ \ |  __|  |  _  /  \ \/ /  |  __|  
    \  /\  / | |__| | |____| |     ____) || |____ | | \ \   \  /   | |____ 
     \/  \/   \____/|______|_|    |_____/ |______||_|  \_\   \/    |______|
                                                                          v{}                                                    
 (C)2025 Wolf Software Systems Ltd - http://wolf.uk.com
"#, VERSION);

    tracing_subscriber::fmt::init();

    // Load configuration
    let config_str = match fs::read_to_string(&config_path).await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Configuration file '{}' not found. Creating default.", config_path);
            let default_config = r#"
[server]
host = "0.0.0.0"
port = 3000

[php]
fpm_address = "127.0.0.1:9993"

[apache]
config_dir = "/etc/apache2"
"#;
            fs::write(&config_path, default_config).await.unwrap();
            default_config.to_string()
        }
    };

    let config: Config = toml::from_str(&config_str).expect("Failed to parse wolfserve.toml");
    
    // Load Apache Virtual Hosts
    let mut vhosts_map = HashMap::new();
    let mut default_vhost: Option<VirtualHost> = None;
    let mut ssl_certs = HashMap::new();
    let mut default_ssl_cert: Option<Arc<CertifiedKey>> = None;
    
    // Collect all ports to listen on
    let mut http_ports = vec![config.server.port]; // Default port
    let mut https_ports = Vec::new();

    let loaded_vhosts = apache::load_apache_config(Path::new(&config.apache.config_dir));
    for vhost in loaded_vhosts {
        let is_ssl = vhost.ssl_cert_file.is_some() && vhost.ssl_key_file.is_some();
        let name_opt = vhost.server_name.clone();

        if is_ssl {
            if !https_ports.contains(&vhost.port) {
                https_ports.push(vhost.port);
                // If this port was previously added as HTTP, remove it
                http_ports.retain(|&p| p != vhost.port);
            }
            match load_ssl_keys(vhost.ssl_cert_file.as_ref().unwrap(), vhost.ssl_key_file.as_ref().unwrap(), vhost.ssl_chain_file.as_ref()) {
                Ok(certified_key) => {
                    let cert_arc = Arc::new(certified_key);
                    if let Some(name) = &name_opt {
                        ssl_certs.insert(name.clone(), cert_arc.clone());
                    } else if default_ssl_cert.is_none() {
                        default_ssl_cert = Some(cert_arc.clone());
                    }
                    for alias in &vhost.server_aliases {
                        ssl_certs.insert(alias.clone(), cert_arc.clone());
                    }
                },
                Err(e) => eprintln!("Failed to load SSL for {:?}: {}", name_opt, e),
            }
        } else {
            // Only add to HTTP ports if it's not already an HTTPS port
            if !http_ports.contains(&vhost.port) && !https_ports.contains(&vhost.port) {
                http_ports.push(vhost.port);
            }
        }

        if let Some(name) = &name_opt {
            println!("Loaded VHost: {} on port {} -> {:?}", name, vhost.port, vhost.document_root);
            vhosts_map.insert(name.clone(), vhost.clone());
            for alias in &vhost.server_aliases {
                vhosts_map.insert(alias.clone(), vhost.clone());
            }
        } else {
            println!("Loaded Default VHost on port {} -> {:?}", vhost.port, vhost.document_root);
            if default_vhost.is_none() {
                default_vhost = Some(vhost.clone());
            }
        }
    }

    // Create shared admin state for statistics and logging
    let admin_state = Arc::new(AdminState::new());

    let state = Arc::new(AppState { 
        config: config.clone(), 
        vhosts: vhosts_map, 
        default_vhost,
        admin_state: admin_state.clone(),
    });
    let app = Router::new()
        .fallback(any(handle_request))
        .layer(CompressionLayer::new())
        .with_state(state.clone());

    let mut tasks = Vec::new();
    let host_ip = config.server.host.clone();

    // Start Admin Dashboard on port 5000 - always bind to all interfaces
    let admin_app = admin_router(admin_state.clone());
    let admin_addr: SocketAddr = "0.0.0.0:5000".parse().unwrap();
    tasks.push(tokio::spawn(async move {
        println!("WolfServe Admin Dashboard listening on {} (login: admin/admin)", admin_addr);
        let listener = tokio::net::TcpListener::bind(&admin_addr).await.unwrap();
        axum::serve(listener, admin_app).await.unwrap();
    }));

    // Start HTTP Listeners
    for port in http_ports {
        let addr: SocketAddr = format!("{}:{}", host_ip, port).parse().unwrap();
        let app_clone = app.clone();
        tasks.push(tokio::spawn(async move {
            println!("WolfServe HTTP listening on {}", addr);
            let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
            axum::serve(listener, app_clone).await.unwrap();
        }));
    }

    // Start HTTPS Listeners
    if !https_ports.is_empty() && (!ssl_certs.is_empty() || default_ssl_cert.is_some()) {
        let resolver = Arc::new(ServerCertResolver { 
            certs: ssl_certs,
            default_cert: default_ssl_cert,
        });
        let mut tls_config_inner = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);
        // Advertise HTTP/2 via ALPN.
        //
        // `hyper_util::server::conn::auto::Builder` below already serves h2 or HTTP/1.1
        // depending on what the connection turns out to be — but over TLS a browser only
        // ever speaks h2 when the server SELECTS it during the handshake (RFC 7301; RFC
        // 9113 s3.3 makes ALPN the sole negotiation mechanism for HTTP/2 over TLS). With
        // no `alpn_protocols` rustls negotiates nothing, so every client fell back to
        // HTTP/1.1 and was capped at ~6 connections per origin.
        //
        // That cap is the second half of the slow-load problem measured on 2026-08-25:
        // the WolfStorm viewer pulls 193 script files, which at 6-way parallelism is ~33
        // serialised round trips before the app can start. h2 multiplexes them over one
        // connection.
        //
        // Order is preference order — h2 first, with http/1.1 retained so any client that
        // does not offer h2 is unaffected. Safe here because wolfserve serves no WebSocket
        // endpoints (WebSockets over h2 need Extended CONNECT, which hyper does not do by
        // default); verified by grep before enabling.
        tls_config_inner.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let tls_config = Arc::new(tls_config_inner);
            
        for port in https_ports {
            let addr: SocketAddr = format!("{}:{}", host_ip, port).parse().unwrap();
            let app_clone = app.clone();
            let tls_config_clone = tls_config.clone();
            
            tasks.push(tokio::spawn(async move {
                println!("WolfServe HTTPS listening on {}", addr);
                let tls_acceptor = TlsAcceptor::from(tls_config_clone);
                let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
                
                loop {
                    let (stream, _) = match listener.accept().await {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    
                    let acceptor = tls_acceptor.clone();
                    let app = app_clone.clone();
                    
                    tokio::spawn(async move {
                         match acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                let io = TokioIo::new(tls_stream);
                                let service = TowerToHyperService { service: app };
                                
                                if let Err(err) = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                                    .serve_connection(io, service)
                                    .await 
                                {
                                    if !is_common_connection_error(err.as_ref()) {
                                        eprintln!("Error serving connection: {:?}", err);
                                    }
                                }
                            }
                            Err(e) => {
                                if !is_common_connection_error(&e) {
                                    eprintln!("TLS Accept Error: {}", e);
                                }
                            }
                         }
                    });

                }
            }));
        }
    }

    join_all(tasks).await;
}


/// Percent-decode a URL path into its on-disk form (e.g. "%20" -> " "). Browsers percent-encode
/// reserved characters in the request line, but the filesystem stores the literal name, so any
/// path with a space (or other encoded byte) must be decoded before we touch disk — otherwise
/// "uploads/Luton%204.jpg" is looked up verbatim and 404s. Uses lossy UTF-8 so it never panics on
/// malformed input; callers still guard the result against ".." traversal.
fn percent_decode_path(path: &str) -> String {
    percent_decode_str(path).decode_utf8_lossy().into_owned()
}

async fn handle_request(State(state): State<Arc<AppState>>, headers: HeaderMap, req: Request) -> Response {
    let start_time = Instant::now();
    let uri_path = req.uri().path().to_string();
    // Decoded form used for ALL filesystem access. uri_path stays raw (encoded) for logging,
    // REQUEST_URI and rewrite/redirect matching, which conventionally operate on the raw path.
    let decoded_uri_path = percent_decode_path(&uri_path);
    let query_string = req.uri().query().unwrap_or("").to_string();
    let method = req.method().to_string();
    
    // Extract info for logging before we consume headers
    let client_ip = headers.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()).map(|s| s.to_string()))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    
    let user_agent = headers.get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    
    let host_for_log = headers.get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    
    // Safety: prevent traversing up. Check the DECODED path so an encoded "%2e%2e" cannot slip
    // past this guard and reach the filesystem as "..".
    let clean_path = decoded_uri_path.trim_start_matches('/');
    if clean_path.contains("..") {
        let response = (StatusCode::FORBIDDEN, "Forbidden").into_response();
        log_request(&state, &method, &uri_path, 403, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
        return response;
    }

    // Determine Document Root and VHost based on Host header
    let mut doc_root = PathBuf::from("public");
    let mut current_vhost: Option<&apache::VirtualHost> = None;
    let mut host_name = String::new();
    
    if let Some(host_header) = headers.get("host") {
        if let Ok(host_str) = host_header.to_str() {
            // Remove port if present
            host_name = host_str.split(':').next().unwrap_or(host_str).to_string();
            if let Some(vhost) = state.vhosts.get(&host_name) {
                current_vhost = Some(vhost);
                if let Some(root) = &vhost.document_root {
                    doc_root = root.clone();
                }
            } else if let Some(vhost) = &state.default_vhost {
                current_vhost = Some(vhost);
                if let Some(root) = &vhost.document_root {
                    doc_root = root.clone();
                }
            }
        }
    } else if let Some(vhost) = &state.default_vhost {
        current_vhost = Some(vhost);
        if let Some(root) = &vhost.document_root {
            doc_root = root.clone();
        }
    }

    // Check for redirects from vhost config first
    if let Some(vhost) = current_vhost {
        for redirect in &vhost.redirects {
            if let Some((status_code, target)) = redirect.matches(&uri_path) {
                let response = handle_redirect(status_code, target);
                log_request(&state, &method, &uri_path, status_code, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
                return response;
            }
        }
    }

    // Check for .htaccess in document root
    let htaccess_path = doc_root.join(".htaccess");
    let mut rewritten_path = uri_path.clone();
    
    if htaccess_path.exists() {
        if let Some(htaccess) = apache::parse_htaccess(&htaccess_path) {
            // Check .htaccess redirects
            for redirect in &htaccess.redirects {
                if let Some((status_code, target)) = redirect.matches(&uri_path) {
                    let response = handle_redirect(status_code, target);
                    log_request(&state, &method, &uri_path, status_code, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
                    return response;
                }
            }
            
            // Check rewrite rules
            let request_filename = doc_root.join(clean_path);
            let is_https = headers.get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .map(|s| s == "https")
                .unwrap_or(false);
            
            let ctx = RewriteContext {
                request_uri: &uri_path,
                request_filename: &request_filename,
                query_string: &query_string,
                http_host: &host_name,
                request_method: &method,
                https: is_https,
                document_root: &doc_root,
            };
            
            if let Some(result) = htaccess.apply_rewrites(&ctx) {
                match result {
                    RewriteResult::Redirect { url, status } => {
                        let response = handle_redirect(status, Some(url));
                        log_request(&state, &method, &uri_path, status, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
                        return response;
                    }
                    RewriteResult::InternalRewrite { path } => {
                        rewritten_path = path;
                    }
                }
            }
        }
    }

    // Use the rewritten path. Decode it for filesystem access — a no-op for server-side rewrite
    // targets like "index.php", and the real decoder when no rewrite applied and this is still
    // the raw URL path.
    let clean_rewritten = rewritten_path.trim_start_matches('/');
    let decoded_rewritten = percent_decode_path(clean_rewritten);
    let mut path = doc_root.join(&decoded_rewritten);

    // Resolve directory index
    if path.is_dir() {
        if path.join("index.php").exists() {
            path = path.join("index.php");
        } else if path.join("index.html").exists() {
            path = path.join("index.html");
        } else {
            let response = (StatusCode::FORBIDDEN, "Directory listing denied").into_response();
            log_request(&state, &method, &uri_path, 403, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
            return response;
        }
    }

    // If file doesn't exist after rewrite, still try to serve (WordPress may handle it)
    if !path.exists() {
        // For WordPress: if we have a rewrite to index.php, use that
        let index_php = doc_root.join("index.php");
        if index_php.exists() && rewritten_path != uri_path {
            // This was an internal rewrite - WordPress will handle routing
            let response = handle_php(state.clone(), req, index_php).await;
            let status = response.status().as_u16();
            log_request(&state, &method, &uri_path, status, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
            return response;
        }
        let response = (StatusCode::NOT_FOUND, "Not Found").into_response();
        log_request(&state, &method, &uri_path, 404, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
        return response;
    }


    if let Some(ext) = path.extension() {
        if ext == "php" {
            let response = handle_php(state.clone(), req, path).await;
            let status = response.status().as_u16();
            log_request(&state, &method, &uri_path, status, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
            return response;
        }
    }

    // Serve static file
    let response = serve_static_file(path, &headers, &query_string).await;
    let status = response.status().as_u16();
    log_request(&state, &method, &uri_path, status, start_time.elapsed().as_millis() as u64, &client_ip, &host_for_log, &user_agent);
    response
}

/// Log a request to the admin state
fn log_request(state: &AppState, method: &str, path: &str, status: u16, duration_ms: u64, client_ip: &str, host: &str, user_agent: &str) {
    let entry = RequestLogEntry {
        timestamp: Utc::now(),
        method: method.to_string(),
        path: path.to_string(),
        status,
        duration_ms,
        client_ip: client_ip.to_string(),
        host: host.to_string(),
        user_agent: user_agent.to_string(),
    };
    state.admin_state.log_request(entry);
}

/// Handle redirect responses based on status code
fn handle_redirect(status_code: u16, target: Option<String>) -> Response {
    let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::FOUND);
    
    match target {
        Some(url) => {
            // Create redirect response with Location header
            let mut response = Response::builder()
                .status(status)
                .header(axum::http::header::LOCATION, &url)
                .body(axum::body::Body::empty())
                .unwrap();
            
            // For 3xx redirects, add a helpful HTML body
            if (300..400).contains(&status_code) {
                let body = format!(
                    "<!DOCTYPE HTML PUBLIC \"-//IETF//DTD HTML 2.0//EN\">\n\
                    <html><head>\n\
                    <title>{} {}</title>\n\
                    </head><body>\n\
                    <h1>{}</h1>\n\
                    <p>The document has moved <a href=\"{}\">here</a>.</p>\n\
                    </body></html>",
                    status_code,
                    status.canonical_reason().unwrap_or("Redirect"),
                    status.canonical_reason().unwrap_or("Redirect"),
                    url
                );
                response = Response::builder()
                    .status(status)
                    .header(axum::http::header::LOCATION, &url)
                    .header(axum::http::header::CONTENT_TYPE, "text/html; charset=iso-8859-1")
                    .body(axum::body::Body::from(body))
                    .unwrap();
            }
            response
        }
        None => {
            // No target URL - likely a 410 Gone response
            let body = format!(
                "<!DOCTYPE HTML PUBLIC \"-//IETF//DTD HTML 2.0//EN\">\n\
                <html><head>\n\
                <title>{} {}</title>\n\
                </head><body>\n\
                <h1>{}</h1>\n\
                <p>The requested resource is no longer available on this server.</p>\n\
                </body></html>",
                status_code,
                status.canonical_reason().unwrap_or("Gone"),
                status.canonical_reason().unwrap_or("Gone")
            );
            Response::builder()
                .status(status)
                .header(axum::http::header::CONTENT_TYPE, "text/html; charset=iso-8859-1")
                .body(axum::body::Body::from(body))
                .unwrap()
        }
    }
}

/// Format a SystemTime as an HTTP IMF-fixdate, e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
///
/// Source: RFC 9110 s5.6.7 — "An HTTP-date value represents time as an instance of
/// Coordinated Universal Time (UTC) [...] preferred format is a fixed-length subset of
/// the format defined in RFC 5322", and senders MUST use IMF-fixdate. chrono formats
/// day/month names in English regardless of locale, which is what the grammar requires.
fn http_date(t: std::time::SystemTime) -> Option<String> {
    let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    let dt = chrono::DateTime::<Utc>::from_timestamp(dur.as_secs() as i64, 0)?;
    Some(dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

/// Parse an HTTP IMF-fixdate back to whole seconds since the epoch.
///
/// Only IMF-fixdate is accepted. RFC 9110 s5.6.7 also lists two obsolete formats that a
/// recipient MUST accept, but they appear only from pre-1995 clients; failing to parse
/// simply means we skip the 304 and send the body, which is always correct if wasteful.
fn parse_http_date(s: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%a, %d %b %Y %H:%M:%S GMT")
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

/// Does an `If-None-Match` field value match our entity tag?
///
/// Source: RFC 9110 s13.1.2 — the value is `*` or a comma-separated list of entity tags,
/// compared with the WEAK comparison function for If-None-Match. Weak comparison ignores
/// the `W/` prefix, so we strip it from both sides before comparing opaque tags.
fn if_none_match_matches(header: &str, etag: &str) -> bool {
    let strip = |t: &str| t.trim().trim_start_matches("W/").trim().to_string();
    if header.trim() == "*" {
        return true;
    }
    let ours = strip(etag);
    header.split(',').any(|candidate| strip(candidate) == ours)
}

/// Serve a file from disk with cache validators and a conditional-request fast path.
///
/// WHY THIS EXISTS (measured 2026-08-25): wolfserve previously sent ONLY `Content-Type`.
/// With no `ETag`, no `Last-Modified` and no `Cache-Control`, a browser cannot revalidate
/// and cannot even apply heuristic freshness (RFC 9111 s4.2.2 needs `Last-Modified` to
/// compute one), so every visit re-downloaded every asset. For the WolfStorm viewer that
/// is 193 script files, 2.28 MB gzipped, measured at 7.33 s on a fast wired link — and it
/// was the single largest contributor to users on slow connections concluding the viewer
/// was broken when it was merely still loading.
///
/// CACHE POLICY, and why it is deliberately conservative:
///   * A URL carrying the `_cb=` content token is immutable — index.php derives that token
///     from the newest mtime across js/, css/ and icons/, so ANY deploy produces new URLs.
///     A stale copy can therefore never be served for a token that is still in use, which
///     is the precondition RFC 8246 requires before `immutable` is safe.
///   * Everything else gets `max-age=0, must-revalidate`: always revalidate, but answer
///     with a 304 instead of the body. That cannot serve anything stale under any
///     circumstance, and still removes the payload from the common case.
///
/// This deliberately does NOT introduce any freshness lifetime for un-tokened URLs. The
/// grid has been bitten before by content pinned in a cache with no user-side recovery
/// (a service worker froze users on a January build for seven months), and an HTTP cache
/// entry is just as unreachable. Revalidation is cheap; staleness is not.
/// Does this Accept-Encoding header genuinely accept Brotli?
///
/// Deliberately not a substring search for "br": that misses the `q=0` case, which is a
/// client explicitly REFUSING the encoding (RFC 9110 s12.5.3 — "a qvalue of 0 means the
/// content-coding is not acceptable"). Serving br to a client that sent `br;q=0` is a
/// broken page, not a slow one.
fn accepts_brotli(headers: &axum::http::HeaderMap) -> bool {
    let raw = match headers
        .get(axum::http::header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v,
        None => return false,
    };
    for part in raw.split(',') {
        let mut it = part.split(';');
        let token = it.next().unwrap_or("").trim();
        if !token.eq_ignore_ascii_case("br") {
            continue;
        }
        for param in it {
            let param = param.trim();
            let lower = param.to_ascii_lowercase();
            if let Some(q) = lower.strip_prefix("q=") {
                if q.trim().parse::<f32>().map(|n| n <= 0.0).unwrap_or(false) {
                    return false;
                }
            }
        }
        return true;
    }
    false
}

async fn serve_static_file(path: PathBuf, headers: &axum::http::HeaderMap, query_string: &str) -> Response {
    use axum::http::header;

    let metadata = match fs::metadata(&path).await {
        Ok(m) => m,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Error reading file").into_response(),
    };

    // ── Precompressed Brotli ──────────────────────────────────────────────────────────
    //
    // WHY (measured 2026-08-25 against this server): the router's CompressionLayer is
    // `CompressionLayer::new()` — DEFAULT quality, which for Brotli is q4. Across the 200 JS
    // files the viewer loads, q4 Brotli totals 2,593,647 bytes against gzip's 2,562,877, so
    // Brotli was actually LOSING to gzip in aggregate while Chrome preferred it. Brotli q11
    // totals 2,117,277 bytes: 446 KB and 17.4% off every cold load.
    //
    // q11 cannot be done per request — it is far slower than q4 and would move the cost from
    // the network onto the CPU on every hit. So it is done once, ahead of time, and served
    // from a `.br` sibling. This is the same mechanism tower-http's ServeDir calls
    // `precompressed_br`; this server has its own static handler, so it needs its own.
    //
    // STALENESS IS THE FAILURE MODE THAT MATTERS. A `.br` left behind by an older deploy would
    // serve OLD JAVASCRIPT to every Brotli-capable client while the plain file looked correct,
    // which is near-undiagnosable from the outside — and this project has been bitten by
    // exactly that shape before (a service worker pinned users to a January build for seven
    // months). So the `.br` is used ONLY when its mtime is >= the source's. Otherwise it is
    // ignored and the layer compresses the fresh original.
    //
    // tower-http skips any response that already carries Content-Encoding
    // (compression/future.rs:43), so this body is not re-compressed. It also appends
    // `Vary: accept-encoding` ONLY when it actually compresses (future.rs:53) — which is why
    // this path must set Vary itself, or a shared cache could hand the Brotli bytes to a
    // client that never asked for them.
    let mut serve_path = path.clone();
    let mut serve_meta = metadata.clone();
    let mut is_br = false;
    if accepts_brotli(headers) {
        let mut br_os = path.clone().into_os_string();
        br_os.push(".br");
        let br_path = PathBuf::from(br_os);
        if let Ok(br_meta) = fs::metadata(&br_path).await {
            let fresh = match (br_meta.modified(), metadata.modified()) {
                (Ok(b), Ok(s)) => b >= s,
                _ => false,
            };
            if fresh && br_meta.len() > 0 {
                serve_path = br_path;
                serve_meta = br_meta;
                is_br = true;
            }
        }
    }
    // Validators must describe the representation actually sent, or a client switching
    // encodings could be handed a 304 for bytes it does not have (RFC 9110 s8.8.1: an
    // entity-tag identifies one specific representation).
    let metadata = serve_meta;

    let modified = metadata.modified().ok();
    let last_modified = modified.and_then(http_date);

    // Entity tag from mtime + size, the same inputs Apache's FileETag MTime+Size uses.
    // Quoted because RFC 9110 s8.8.3 defines entity-tag as a quoted opaque string.
    let etag = modified
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format!("\"{:x}-{:x}\"", d.as_secs(), metadata.len()));

    // A `_cb=` token means the URL is content-addressed: see the policy note above.
    let is_content_tokened = query_string
        .split('&')
        .any(|pair| pair.starts_with("_cb=") && pair.len() > 4);
    let cache_control = if is_content_tokened {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=0, must-revalidate"
    };

    // --- Conditional request handling -------------------------------------------------
    // Source: RFC 9110 s13.2.2 — If-None-Match takes precedence; a recipient MUST ignore
    // If-Modified-Since when If-None-Match is present.
    let mut not_modified = false;
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if let Some(ref tag) = etag {
            not_modified = if_none_match_matches(inm, tag);
        }
    } else if let Some(ims) = headers.get(header::IF_MODIFIED_SINCE).and_then(|v| v.to_str().ok()) {
        if let (Some(since), Some(m)) = (parse_http_date(ims), modified) {
            if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                // Whole-second granularity: not modified when mtime <= the supplied date.
                not_modified = (d.as_secs() as i64) <= since;
            }
        }
    }

    // Source: RFC 9110 s15.4.5 — a 304 MUST include the validators that would have been
    // sent on a 200, and carries no body.
    let mut builder = Response::builder();
    if let Some(ref tag) = etag {
        builder = builder.header(header::ETAG, tag.clone());
    }
    if let Some(ref lm) = last_modified {
        builder = builder.header(header::LAST_MODIFIED, lm.clone());
    }
    builder = builder.header(header::CACHE_CONTROL, cache_control);
    if is_br {
        // Set on the 304 as well as the 200: RFC 9110 s15.4.5 requires a 304 to carry the
        // header fields that would have been sent on a 200, and a cache that stored the
        // response without Vary would serve it to clients that cannot decode it.
        builder = builder
            .header(header::CONTENT_ENCODING, "br")
            .header(header::VARY, "accept-encoding");
    }

    if not_modified {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(axum::body::Body::empty())
            .unwrap()
            .into_response();
    }

    match fs::read(&serve_path).await {
        Ok(content) => {
            // MIME comes from the ORIGINAL path, never from serve_path: `main.js.br` guesses as
            // application/octet-stream, and a script served with that Content-Type is refused
            // outright by browsers under X-Content-Type-Options: nosniff. Content-Encoding
            // describes the transfer coding; Content-Type must still describe the payload.
            let mime_type = mime_guess::from_path(&path).first_or_text_plain();
            builder
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_type.to_string())
                .body(axum::body::Body::from(content))
                .unwrap()
                .into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error reading file").into_response(),
    }
}

async fn handle_php(state: Arc<AppState>, req: Request, script_path: PathBuf) -> Response {
    if state.config.php.mode == "cgi" {
        return handle_php_cgi(state, req, script_path).await;
    }
    handle_php_fpm(state, req, script_path).await
}

async fn handle_php_cgi(state: Arc<AppState>, req: Request, script_path: PathBuf) -> Response {
    let mut cmd = tokio::process::Command::new(&state.config.php.cgi_path);
    
    let script_filename = match std::fs::canonicalize(&script_path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return (StatusCode::NOT_FOUND, "Script not found on disk").into_response(),
    };

    cmd.env("REDIRECT_STATUS", "200")
       .env("SCRIPT_FILENAME", script_filename)
       .env("SCRIPT_NAME", req.uri().path())
       .env("REQUEST_METHOD", req.method().as_str())
       .env("SERVER_SOFTWARE", format!("wolfserve/{}", VERSION))
       .env("REMOTE_ADDR", "127.0.0.1")
       .env("SERVER_PROTOCOL", "HTTP/1.1");
       
    if let Some(query) = req.uri().query() {
        cmd.env("QUERY_STRING", query);
    }
    
    for (name, value) in req.headers() {
         let key = format!("HTTP_{}", name.as_str().replace('-', "_").to_uppercase());
         if let Ok(val) = value.to_str() {
             cmd.env(key, val);
         }
         if name == "content-type" {
             if let Ok(val) = value.to_str() { cmd.env("CONTENT_TYPE", val); }
         }
         if name == "content-length" {
             if let Ok(val) = value.to_str() { cmd.env("CONTENT_LENGTH", val); }
         }
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to spawn php-cgi: {}", e)).into_response(),
    };

    let (_parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read body").into_response(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(_) = stdin.write_all(&body_bytes).await {
             // Ignore write error
        }
    }

    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to wait for php-cgi: {}", e)).into_response(),
    };
    
    if !output.stderr.is_empty() {
        eprintln!("PHP CGI Error: {}", String::from_utf8_lossy(&output.stderr));
    }

    parse_php_response(output.stdout)
}

async fn handle_php_fpm(state: Arc<AppState>, req: Request, script_path: PathBuf) -> Response {
    let fpm_addr = match &state.config.php.fpm_address {
        Some(addr) => addr,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "PHP-FPM address not configured").into_response(),
    };

    // Basic FastCGI connection to PHP-FPM with timeout and optional Unix socket support
    let fpm_connect_timeout = Duration::from_secs(2);

    enum StreamKind {
        Tcp(TcpStream),
        Unix(UnixStream),
    }

    let stream = if let Some(path) = fpm_addr.strip_prefix("unix:") {
        match timeout(fpm_connect_timeout, UnixStream::connect(path)).await {
            Ok(Ok(s)) => StreamKind::Unix(s),
            Ok(Err(e)) => return (StatusCode::BAD_GATEWAY, format!("PHP-FPM unreachable at unix:{}: {}", path, e)).into_response(),
            Err(_) => return (StatusCode::GATEWAY_TIMEOUT, format!("PHP-FPM connect timed out (unix:{})", path)).into_response(),
        }
    } else {
        match timeout(fpm_connect_timeout, TcpStream::connect(fpm_addr)).await {
            Ok(Ok(s)) => StreamKind::Tcp(s),
            Ok(Err(e)) => return (StatusCode::BAD_GATEWAY, format!("PHP-FPM unreachable at {}: {}", fpm_addr, e)).into_response(),
            Err(_) => return (StatusCode::GATEWAY_TIMEOUT, format!("PHP-FPM connect timed out ({})", fpm_addr)).into_response(),
        }
    };

    // Read body
    let (parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read body").into_response(),
    };

    let script_filename = match std::fs::canonicalize(&script_path) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return (StatusCode::NOT_FOUND, "Script not found on disk").into_response(),
    };

    // Construct FastCGI params
    let mut params = Params::default();
    params.insert(Cow::Borrowed("REQUEST_METHOD"), Cow::Owned(parts.method.as_str().to_string()));
    params.insert(Cow::Borrowed("SCRIPT_FILENAME"), Cow::Owned(script_filename));
    params.insert(Cow::Borrowed("SCRIPT_NAME"), Cow::Owned(parts.uri.path().to_string()));
    params.insert(Cow::Borrowed("REQUEST_URI"), Cow::Owned(parts.uri.path_and_query().map(|pq| pq.to_string()).unwrap_or_else(|| parts.uri.path().to_string())));
    params.insert(Cow::Borrowed("QUERY_STRING"), Cow::Owned(parts.uri.query().unwrap_or("").to_string()));
    params.insert(Cow::Borrowed("SERVER_SOFTWARE"), Cow::Owned(format!("wolfserve/{}", VERSION)));
    params.insert(Cow::Borrowed("SERVER_PROTOCOL"), Cow::Borrowed("HTTP/1.1"));
    params.insert(Cow::Borrowed("GATEWAY_INTERFACE"), Cow::Borrowed("CGI/1.1"));
    
    // Handle proxy headers for real client IP
    let remote_addr = parts.headers.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| parts.headers.get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    params.insert(Cow::Borrowed("REMOTE_ADDR"), Cow::Owned(remote_addr));
    
    // Handle HTTPS detection for proxied requests
    let is_https = parts.headers.get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("https"))
        .unwrap_or(false);
    if is_https {
        params.insert(Cow::Borrowed("HTTPS"), Cow::Borrowed("on"));
    }
    
    // Server name from Host header
    if let Some(host) = parts.headers.get("host") {
        if let Ok(host_str) = host.to_str() {
            let server_name = host_str.split(':').next().unwrap_or(host_str);
            params.insert(Cow::Borrowed("SERVER_NAME"), Cow::Owned(server_name.to_string()));
            params.insert(Cow::Borrowed("HTTP_HOST"), Cow::Owned(host_str.to_string()));
        }
    }
    
    // Handle headers.
    //
    // [FIX 2026-08-25] A repeated header must be RECOMBINED into one FastCGI param, not
    // overwritten. `params.insert` replaced any earlier value for the same key, so when a
    // client sent a header more than once only the LAST occurrence reached PHP. This bit
    // Cookie hardest: under HTTP/2 (which we now negotiate) clients are encouraged to split
    // the cookie list into several `cookie` header fields (RFC 7540 §8.1.2.5), and Chromium
    // does exactly that — so PHP received only the last field and dropped PHPSESSID, logging
    // the user out on every click. Firefox sends a single `cookie` field, which is why it was
    // unaffected. RFC 3875 §4.1.18 requires repeated headers be joined with ", ", except the
    // Cookie header, whose own grammar joins with "; ".
    let mut header_params: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, value) in parts.headers.iter() {
        let Ok(val) = value.to_str() else { continue };
        let key = format!("HTTP_{}", name.as_str().replace('-', "_").to_uppercase());
        let sep = if name == axum::http::header::COOKIE { "; " } else { ", " };
        header_params
            .entry(key)
            .and_modify(|existing| {
                existing.push_str(sep);
                existing.push_str(val);
            })
            .or_insert_with(|| val.to_string());
    }
    for (key, val) in header_params {
        params.insert(Cow::Owned(key), Cow::Owned(val));
    }
    
    // Content Headers
    if let Some(ct) = parts.headers.get("content-type") {
        if let Ok(v) = ct.to_str() {
             params.insert(Cow::Borrowed("CONTENT_TYPE"), Cow::Owned(v.to_string()));
        }
    }
    if let Some(cl) = parts.headers.get("content-length") {
        if let Ok(v) = cl.to_str() {
             params.insert(Cow::Borrowed("CONTENT_LENGTH"), Cow::Owned(v.to_string()));
        }
    }

    let fcgi_req = FcgiRequest::new(params, &body_bytes[..]);

    let output = match stream {
        StreamKind::Tcp(s) => {
            let client = Client::new(s);
            match client.execute_once(fcgi_req).await {
                Ok(o) => o,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("FastCGI Error: {}", e)).into_response(),
            }
        }
        StreamKind::Unix(s) => {
            let client = Client::new(s);
            match client.execute_once(fcgi_req).await {
                Ok(o) => o,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("FastCGI Error: {}", e)).into_response(),
            }
        }
    };

    let stdout = match output.stdout {
        Some(s) => s,
        None => return (StatusCode::INTERNAL_SERVER_ERROR, "PHP output is empty").into_response(),
    };
    
    parse_php_response(stdout)
}

fn parse_php_response(stdout: Vec<u8>) -> Response {
    let mut status_code = StatusCode::OK;
    let mut headers = HeaderMap::new();

    let split_indices = stdout.windows(4).position(|window| window == b"\r\n\r\n");
    
    let body_data = if let Some(idx) = split_indices {
        let header_part = &stdout[0..idx];
        let body_part = &stdout[idx+4..];
        
        if let Ok(header_str) = std::str::from_utf8(header_part) {
            for line in header_str.split("\r\n") {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim();
                    if key.eq_ignore_ascii_case("Status") {
                         if let Some(code_str) = value.split_whitespace().next() {
                             if let Ok(code) = code_str.parse::<u16>() {
                                 if let Ok(s) = StatusCode::from_u16(code) {
                                     status_code = s;
                                 }
                             }
                         }
                    } else {
                        if let Ok(hname) = axum::http::header::HeaderName::from_bytes(key.as_bytes()) {
                            if let Ok(hval) = axum::http::header::HeaderValue::from_str(value) {
                                // Use append for Set-Cookie to allow multiple cookies
                                // (insert would replace previous values)
                                if hname == axum::http::header::SET_COOKIE {
                                    headers.append(hname, hval);
                                } else {
                                    headers.insert(hname, hval);
                                }
                            }
                        }
                    }
                }
            }
        }
        body_part.to_vec()
    } else {
        stdout
    };

    (status_code, headers, body_data).into_response()
}

#[cfg(test)]
mod tests {
    use super::percent_decode_path;

    #[test]
    fn decodes_spaces_so_static_files_resolve() {
        // The bug: spaced uploads 404'd because "%20" was never decoded before disk lookup.
        assert_eq!(percent_decode_path("/uploads/Luton%204.jpg"), "/uploads/Luton 4.jpg");
        assert_eq!(
            percent_decode_path("/uploads/WhatsApp%20Image%202026-04-24%20at%2009.05.48.jpeg"),
            "/uploads/WhatsApp Image 2026-04-24 at 09.05.48.jpeg"
        );
    }

    #[test]
    fn plain_paths_are_unchanged() {
        assert_eq!(percent_decode_path("/uploads/i2mage.png"), "/uploads/i2mage.png");
        assert_eq!(percent_decode_path("/index.php"), "/index.php");
    }

    #[test]
    fn encoded_traversal_is_revealed_for_the_dotdot_guard() {
        // Decoding must happen BEFORE the ".." check so encoded traversal can't slip through.
        let decoded = percent_decode_path("/%2e%2e/%2e%2e/etc/passwd");
        assert!(decoded.contains(".."), "decoded form must expose '..' to the guard: {decoded}");
    }
}
