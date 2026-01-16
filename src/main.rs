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
use tokio::time::{timeout, Duration};
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

mod apache;
use apache::VirtualHost;
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

#[tokio::main]
async fn main() {
    println!(r#"
 __          ______  _      ______  _____  ______  _____ __      __ ______ 
 \ \        / / __ \| |    |  ____|/ ____||  ____||  __ \\ \    / /|  ____|
  \ \  /\  / / |  | | |    | |__  | (___  | |__   | |__) |\ \  / / | |__   
   \ \/  \/ /| |  | | |    |  __|  \___ \ |  __|  |  _  /  \ \/ /  |  __|  
    \  /\  / | |__| | |____| |     ____) || |____ | | \ \   \  /   | |____ 
     \/  \/   \____/|______|_|    |_____/ |______||_|  \_\   \/    |______|
                                                                
 (C)2025 Wolf Software Systems Ltd - http://wolf.uk.com
"#);

    tracing_subscriber::fmt::init();

    // Load configuration
    let config_str = match fs::read_to_string("wolfserve.toml").await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Configuration file 'wolfserve.toml' not found. Creating default.");
            let default_config = r#"
[server]
host = "0.0.0.0"
port = 3000

[php]
fpm_address = "127.0.0.1:9993"

[apache]
config_dir = "/etc/apache2"
"#;
            fs::write("wolfserve.toml", default_config).await.unwrap();
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

    let state = Arc::new(AppState { 
        config: config.clone(), 
        vhosts: vhosts_map, 
        default_vhost 
    });
    let app = Router::new()
        .fallback(any(handle_request))
        .with_state(state.clone());

    let mut tasks = Vec::new();
    let host_ip = config.server.host.clone();

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
        let tls_config = Arc::new(rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver));
            
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


async fn handle_request(State(state): State<Arc<AppState>>, headers: HeaderMap, req: Request) -> Response {
    let uri_path = req.uri().path().to_string();
    
    // Safety: prevent traversing up
    let clean_path = uri_path.trim_start_matches('/');
    if clean_path.contains("..") {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    // Determine Document Root based on Host header
    let mut doc_root = PathBuf::from("public");
    if let Some(host_header) = headers.get("host") {
        if let Ok(host_str) = host_header.to_str() {
            // Remove port if present
            let host_name = host_str.split(':').next().unwrap_or(host_str);
            if let Some(vhost) = state.vhosts.get(host_name) {
                if let Some(root) = &vhost.document_root {
                    doc_root = root.clone();
                }
            } else if let Some(vhost) = &state.default_vhost {
                if let Some(root) = &vhost.document_root {
                    doc_root = root.clone();
                }
            }
        }
    } else if let Some(vhost) = &state.default_vhost {
        if let Some(root) = &vhost.document_root {
            doc_root = root.clone();
        }
    }

    let mut path = doc_root.join(clean_path);

    // Resolve directory index
    if path.is_dir() {
        if path.join("index.php").exists() {
            path = path.join("index.php");
        } else if path.join("index.html").exists() {
            path = path.join("index.html");
        } else {
             return (StatusCode::FORBIDDEN, "Directory listing denied").into_response();
        }
    }

    if !path.exists() {
         return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }


    if let Some(ext) = path.extension() {
        if ext == "php" {
            return handle_php(state, req, path).await;
        }
    }

    // Serve static file
    serve_static_file(path).await
}

async fn serve_static_file(path: PathBuf) -> Response {
    match fs::read(&path).await {
        Ok(content) => {
            let mime_type = mime_guess::from_path(&path).first_or_text_plain();
            (
                [(axum::http::header::CONTENT_TYPE, mime_type.to_string())],
                content,
            ).into_response()
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
       .env("SERVER_SOFTWARE", "wolfserve/0.1.0")
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
    params.insert(Cow::Borrowed("QUERY_STRING"), Cow::Owned(parts.uri.query().unwrap_or("").to_string()));
    params.insert(Cow::Borrowed("SERVER_SOFTWARE"), Cow::Borrowed("wolfserve/0.1.0"));
    params.insert(Cow::Borrowed("REMOTE_ADDR"), Cow::Borrowed("127.0.0.1")); 
    params.insert(Cow::Borrowed("SERVER_PROTOCOL"), Cow::Borrowed("HTTP/1.1"));
    
    // Handle headers
    for (name, value) in parts.headers.iter() {
        let key = format!("HTTP_{}", name.as_str().replace('-', "_").to_uppercase());
        if let Ok(val) = value.to_str() {
             params.insert(Cow::Owned(key), Cow::Owned(val.to_string()));
        }
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
                                headers.insert(hname, hval);
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
