use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualHost {
    pub port: u16,
    pub server_name: Option<String>,
    pub server_aliases: Vec<String>,
    pub document_root: Option<PathBuf>,
    pub ssl_cert_file: Option<PathBuf>,
    pub ssl_key_file: Option<PathBuf>,
    pub ssl_chain_file: Option<PathBuf>,
}

pub fn load_apache_config(config_dir: &Path) -> Vec<VirtualHost> {

    let mut vhosts = Vec::new();
    let sites_enabled = config_dir.join("sites-enabled");

    if !sites_enabled.exists() {
        return vhosts;
    }

    if let Ok(entries) = fs::read_dir(sites_enabled) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "conf") {
                vhosts.extend(parse_apache_file(&path, config_dir));
            }
        }
    }
    vhosts
}

fn parse_apache_file(path: &Path, base_dir: &Path) -> Vec<VirtualHost> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut vhosts = Vec::new();
    let mut current_vhost: Option<VirtualHost> = None;

    for line in content.lines() {
        let line = line.trim();
        
        if line.starts_with("<VirtualHost") {
            // Parse port from <VirtualHost *:8080>
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(addr_port) = parts.get(1) {
                let port_str = addr_port.split(':').last().unwrap_or("80");
                let port = port_str.trim_end_matches('>').parse().unwrap_or(80);
                
                current_vhost = Some(VirtualHost {
                    port,
                    server_name: None,
                    server_aliases: Vec::new(),
                    document_root: None,
                    ssl_cert_file: None,
                    ssl_key_file: None,
                    ssl_chain_file: None,
                });
            }
        } else if line.starts_with("</VirtualHost>") {
            if let Some(vhost) = current_vhost.take() {
                vhosts.push(vhost);
            }
        } else if let Some(vhost) = &mut current_vhost {
            if line.starts_with("ServerName") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    vhost.server_name = Some(parts[1].to_string());
                }
            } else if line.starts_with("ServerAlias") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                for part in parts.iter().skip(1) {
                    vhost.server_aliases.push(part.to_string());
                }
            } else if line.starts_with("DocumentRoot") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    vhost.document_root = Some(PathBuf::from(parts[1].trim_matches('"')));
                }
            } else if line.starts_with("SSLCertificateFile") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let p = PathBuf::from(parts[1].trim_matches('"'));
                    vhost.ssl_cert_file = Some(if p.is_absolute() { p } else { base_dir.join(p) });
                }
            } else if line.starts_with("SSLCertificateKeyFile") {
                 let parts: Vec<&str> = line.split_whitespace().collect();
                 if parts.len() >= 2 {
                     let p = PathBuf::from(parts[1].trim_matches('"'));
                     vhost.ssl_key_file = Some(if p.is_absolute() { p } else { base_dir.join(p) });
                 }
            } else if line.starts_with("SSLCertificateChainFile") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let p = PathBuf::from(parts[1].trim_matches('"'));
                    vhost.ssl_chain_file = Some(if p.is_absolute() { p } else { base_dir.join(p) });
                }
            }
        }
    }


    vhosts
}
