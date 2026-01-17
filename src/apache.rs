use std::path::{Path, PathBuf};
use std::fs;
use serde::{Deserialize, Serialize};
use regex::Regex;
use std::collections::HashMap;

/// Represents a redirect rule parsed from Apache config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectRule {
    /// HTTP status code for redirect (301, 302, 303, 307, 308, 410 gone, 451 unavailable)
    pub status: u16,
    /// URL path to match (exact match for Redirect, regex pattern for RedirectMatch)
    pub from: String,
    /// Target URL to redirect to (can include backreferences for RedirectMatch)
    pub to: Option<String>,
    /// Whether this is a regex-based redirect (RedirectMatch)
    pub is_regex: bool,
}

/// Condition for a rewrite rule (RewriteCond)
#[derive(Debug, Clone)]
pub struct RewriteCond {
    /// Test string (e.g., %{REQUEST_FILENAME}, %{REQUEST_URI})
    pub test_string: String,
    /// Condition pattern
    pub pattern: String,
    /// Negate the condition
    pub negate: bool,
    /// Flags: [NC] = nocase, [OR] = or with next condition
    pub nocase: bool,
    pub or_next: bool,
}

/// A rewrite rule (RewriteRule)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RewriteRule {
    /// Pattern to match against the URL path
    pub pattern: String,
    /// Substitution string (- means no substitution)
    pub substitution: String,
    /// Conditions that must be met
    pub conditions: Vec<RewriteCond>,
    /// Flags
    pub last: bool,          // [L] - stop processing
    pub redirect: Option<u16>, // [R], [R=301], [R=302]
    pub nocase: bool,        // [NC]
    pub qsappend: bool,      // [QSA] - query string append
    pub passthrough: bool,   // [PT] - pass through
    pub skip: bool,          // Used internally for "-" substitution
}

/// Parsed .htaccess configuration
#[derive(Debug, Clone, Default)]
pub struct HtaccessConfig {
    pub rewrite_engine: bool,
    pub rewrite_base: String,
    pub rewrite_rules: Vec<RewriteRule>,
    pub redirects: Vec<RedirectRule>,
}

/// Request context for evaluating rewrite conditions
pub struct RewriteContext<'a> {
    pub request_uri: &'a str,
    pub request_filename: &'a Path,
    pub query_string: &'a str,
    pub http_host: &'a str,
    pub request_method: &'a str,
    pub https: bool,
    pub document_root: &'a Path,
}

impl HtaccessConfig {
    /// Apply rewrite rules and return the rewritten path (or None if no rewrite)
    pub fn apply_rewrites(&self, ctx: &RewriteContext) -> Option<RewriteResult> {
        if !self.rewrite_engine {
            return None;
        }

        let mut current_uri = ctx.request_uri.to_string();
        
        // Strip rewrite base from the beginning for matching
        let match_path = if !self.rewrite_base.is_empty() && self.rewrite_base != "/" {
            current_uri.strip_prefix(&self.rewrite_base)
                .unwrap_or(&current_uri)
                .trim_start_matches('/')
                .to_string()
        } else {
            current_uri.trim_start_matches('/').to_string()
        };

        for rule in &self.rewrite_rules {
            // Check conditions
            if !self.evaluate_conditions(&rule.conditions, ctx, &current_uri) {
                continue;
            }

            // Try to match the pattern
            let pattern = if rule.nocase {
                format!("(?i){}", &rule.pattern)
            } else {
                rule.pattern.clone()
            };

            let re = match Regex::new(&pattern) {
                Ok(r) => r,
                Err(_) => continue,
            };

            if let Some(caps) = re.captures(&match_path) {
                // Check for skip (substitution is "-")
                if rule.substitution == "-" {
                    if rule.last {
                        break;
                    }
                    continue;
                }

                // Build substitution with backreferences
                let mut new_uri = rule.substitution.clone();
                for i in 0..=9 {
                    if let Some(m) = caps.get(i) {
                        new_uri = new_uri.replace(&format!("${}", i), m.as_str());
                    }
                }

                // Handle absolute URLs (external redirects)
                if new_uri.starts_with("http://") || new_uri.starts_with("https://") {
                    let status = rule.redirect.unwrap_or(302);
                    return Some(RewriteResult::Redirect { 
                        url: new_uri, 
                        status 
                    });
                }

                // Prepend rewrite base if not absolute path
                if !new_uri.starts_with('/') {
                    new_uri = format!("{}{}", self.rewrite_base, new_uri);
                }

                // Handle query string
                if rule.qsappend && !ctx.query_string.is_empty() {
                    if new_uri.contains('?') {
                        new_uri = format!("{}&{}", new_uri, ctx.query_string);
                    } else {
                        new_uri = format!("{}?{}", new_uri, ctx.query_string);
                    }
                }

                // Check if this is a redirect
                if let Some(status) = rule.redirect {
                    return Some(RewriteResult::Redirect { 
                        url: new_uri, 
                        status 
                    });
                }

                current_uri = new_uri;

                if rule.last {
                    break;
                }
            }
        }

        if current_uri != ctx.request_uri {
            Some(RewriteResult::InternalRewrite { path: current_uri })
        } else {
            None
        }
    }

    fn evaluate_conditions(&self, conditions: &[RewriteCond], ctx: &RewriteContext, current_uri: &str) -> bool {
        if conditions.is_empty() {
            return true;
        }

        let mut result = true;
        let mut or_chain = false;

        for cond in conditions {
            let test_value = self.expand_variables(&cond.test_string, ctx, current_uri);
            let matched = self.test_condition(&test_value, &cond.pattern, cond.nocase);
            let matched = if cond.negate { !matched } else { matched };

            if or_chain {
                result = result || matched;
            } else {
                result = result && matched;
            }

            or_chain = cond.or_next;
        }

        result
    }

    fn expand_variables(&self, s: &str, ctx: &RewriteContext, current_uri: &str) -> String {
        let mut result = s.to_string();
        
        // Common Apache server variables
        result = result.replace("%{REQUEST_URI}", current_uri);
        result = result.replace("%{REQUEST_FILENAME}", &ctx.request_filename.to_string_lossy());
        result = result.replace("%{QUERY_STRING}", ctx.query_string);
        result = result.replace("%{HTTP_HOST}", ctx.http_host);
        result = result.replace("%{REQUEST_METHOD}", ctx.request_method);
        result = result.replace("%{DOCUMENT_ROOT}", &ctx.document_root.to_string_lossy());
        result = result.replace("%{HTTPS}", if ctx.https { "on" } else { "off" });
        
        result
    }

    fn test_condition(&self, test_value: &str, pattern: &str, nocase: bool) -> bool {
        // Special file/directory tests
        match pattern {
            "-f" => return Path::new(test_value).is_file(),
            "-d" => return Path::new(test_value).is_dir(),
            "-s" => return Path::new(test_value).metadata().map(|m| m.len() > 0).unwrap_or(false),
            "-l" => return Path::new(test_value).is_symlink(),
            "-F" => return Path::new(test_value).exists(),
            _ => {}
        }

        // Regex match
        let pattern = if nocase {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };

        Regex::new(&pattern)
            .map(|re| re.is_match(test_value))
            .unwrap_or(false)
    }
}

/// Result of applying rewrite rules
#[derive(Debug, Clone)]
pub enum RewriteResult {
    /// Internal rewrite - serve different path
    InternalRewrite { path: String },
    /// External redirect
    Redirect { url: String, status: u16 },
}

/// Cache for parsed .htaccess files
#[allow(dead_code)]
pub type HtaccessCache = HashMap<PathBuf, HtaccessConfig>;

/// Parse an .htaccess file
pub fn parse_htaccess(path: &Path) -> Option<HtaccessConfig> {
    let content = fs::read_to_string(path).ok()?;
    Some(parse_htaccess_content(&content))
}

/// Parse .htaccess content
pub fn parse_htaccess_content(content: &str) -> HtaccessConfig {
    let mut config = HtaccessConfig {
        rewrite_engine: false,
        rewrite_base: "/".to_string(),
        rewrite_rules: Vec::new(),
        redirects: Vec::new(),
    };

    let mut pending_conditions: Vec<RewriteCond> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        
        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Skip IfModule directives (assume modules are available)
        if line.starts_with("<IfModule") || line.starts_with("</IfModule") {
            continue;
        }

        if line.eq_ignore_ascii_case("RewriteEngine On") {
            config.rewrite_engine = true;
        } else if line.eq_ignore_ascii_case("RewriteEngine Off") {
            config.rewrite_engine = false;
        } else if line.starts_with("RewriteBase") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                config.rewrite_base = parts[1].to_string();
            }
        } else if line.starts_with("RewriteCond") {
            if let Some(cond) = parse_rewrite_cond(line) {
                pending_conditions.push(cond);
            }
        } else if line.starts_with("RewriteRule") {
            if let Some(mut rule) = parse_rewrite_rule(line) {
                rule.conditions = std::mem::take(&mut pending_conditions);
                config.rewrite_rules.push(rule);
            }
        } else if line.starts_with("Redirect") {
            // Handle Redirect directives in .htaccess
            if line.starts_with("RedirectMatch") {
                if let Some(rule) = parse_redirect_directive(line, true) {
                    config.redirects.push(rule);
                }
            } else if line.starts_with("RedirectPermanent") {
                let parts: Vec<&str> = line.splitn(3, char::is_whitespace)
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() >= 3 {
                    config.redirects.push(RedirectRule {
                        status: 301,
                        from: parts[1].to_string(),
                        to: Some(parts[2].to_string()),
                        is_regex: false,
                    });
                }
            } else if line.starts_with("Redirect ") {
                if let Some(rule) = parse_redirect_directive(line, false) {
                    config.redirects.push(rule);
                }
            }
        }
    }

    config
}

fn parse_rewrite_cond(line: &str) -> Option<RewriteCond> {
    // RewriteCond TestString CondPattern [flags]
    let parts: Vec<&str> = line.splitn(4, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    
    if parts.len() < 3 {
        return None;
    }

    let test_string = parts[1].to_string();
    let mut pattern = parts[2].to_string();
    let negate = pattern.starts_with('!');
    if negate {
        pattern = pattern[1..].to_string();
    }

    let mut nocase = false;
    let mut or_next = false;

    if parts.len() >= 4 {
        let flags = parts[3].to_uppercase();
        nocase = flags.contains("NC");
        or_next = flags.contains("OR");
    }

    Some(RewriteCond {
        test_string,
        pattern,
        negate,
        nocase,
        or_next,
    })
}

fn parse_rewrite_rule(line: &str) -> Option<RewriteRule> {
    // RewriteRule Pattern Substitution [flags]
    let parts: Vec<&str> = line.splitn(4, char::is_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    
    if parts.len() < 3 {
        return None;
    }

    let pattern = parts[1].to_string();
    let substitution = parts[2].to_string();
    let skip = substitution == "-";

    let mut last = false;
    let mut redirect = None;
    let mut nocase = false;
    let mut qsappend = false;
    let mut passthrough = false;

    if parts.len() >= 4 {
        let flags = parts[3].to_uppercase();
        last = flags.contains('L') || flags.contains("[L]") || flags.contains("L,") || flags.contains(",L");
        nocase = flags.contains("NC");
        qsappend = flags.contains("QSA");
        passthrough = flags.contains("PT");
        
        // Parse redirect flag [R] or [R=301]
        if flags.contains('R') {
            if let Some(start) = flags.find("R=") {
                let rest = &flags[start + 2..];
                let code_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                redirect = code_str.parse().ok();
            }
            if redirect.is_none() {
                redirect = Some(302); // Default redirect status
            }
        }
    }

    Some(RewriteRule {
        pattern,
        substitution,
        conditions: Vec::new(),
        last,
        redirect,
        nocase,
        qsappend,
        passthrough,
        skip,
    })
}

impl RedirectRule {
    /// Check if this rule matches the given path and return the redirect target
    pub fn matches(&self, path: &str) -> Option<(u16, Option<String>)> {
        if self.is_regex {
            if let Ok(re) = Regex::new(&self.from) {
                if let Some(caps) = re.captures(path) {
                    if let Some(ref to) = self.to {
                        // Replace backreferences $1, $2, etc.
                        let mut target = to.clone();
                        for i in 1..=9 {
                            if let Some(m) = caps.get(i) {
                                target = target.replace(&format!("${}", i), m.as_str());
                            }
                        }
                        return Some((self.status, Some(target)));
                    } else {
                        // Gone or similar - no target
                        return Some((self.status, None));
                    }
                }
            }
        } else {
            // Exact prefix match for regular Redirect
            if path == self.from || path.starts_with(&format!("{}/", self.from)) {
                if let Some(ref to) = self.to {
                    // Append the remainder of the path
                    let remainder = &path[self.from.len()..];
                    let target = format!("{}{}", to, remainder);
                    return Some((self.status, Some(target)));
                } else {
                    return Some((self.status, None));
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualHost {
    pub port: u16,
    pub server_name: Option<String>,
    pub server_aliases: Vec<String>,
    pub document_root: Option<PathBuf>,
    pub ssl_cert_file: Option<PathBuf>,
    pub ssl_key_file: Option<PathBuf>,
    pub ssl_chain_file: Option<PathBuf>,
    pub redirects: Vec<RedirectRule>,
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
                    redirects: Vec::new(),
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
            } else if line.starts_with("RedirectMatch") {
                // RedirectMatch [status] regex-pattern target-URL
                if let Some(rule) = parse_redirect_directive(line, true) {
                    vhost.redirects.push(rule);
                }
            } else if line.starts_with("RedirectPermanent") {
                // RedirectPermanent URL-path URL (shorthand for 301)
                let parts: Vec<&str> = line.splitn(3, char::is_whitespace)
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() >= 3 {
                    vhost.redirects.push(RedirectRule {
                        status: 301,
                        from: parts[1].to_string(),
                        to: Some(parts[2].to_string()),
                        is_regex: false,
                    });
                }
            } else if line.starts_with("RedirectTemp") {
                // RedirectTemp URL-path URL (shorthand for 302)
                let parts: Vec<&str> = line.splitn(3, char::is_whitespace)
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() >= 3 {
                    vhost.redirects.push(RedirectRule {
                        status: 302,
                        from: parts[1].to_string(),
                        to: Some(parts[2].to_string()),
                        is_regex: false,
                    });
                }
            } else if line.starts_with("Redirect") && !line.starts_with("Redirect ") {
                // Other Redirect variants we don't recognize - skip
            } else if line.starts_with("Redirect ") {
                // Redirect [status] URL-path URL
                if let Some(rule) = parse_redirect_directive(line, false) {
                    vhost.redirects.push(rule);
                }
            }
        }
    }


    vhosts
}

/// Parse Apache Redirect or RedirectMatch directive
fn parse_redirect_directive(line: &str, is_regex: bool) -> Option<RedirectRule> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    
    // Minimum: Redirect /path URL or RedirectMatch pattern URL
    if parts.len() < 3 {
        return None;
    }
    
    // Check if second token is a status code or keyword
    let (status, from_idx) = match parts[1] {
        "permanent" | "301" => (301, 2),
        "temp" | "302" => (302, 2),
        "seeother" | "303" => (303, 2),
        "gone" | "410" => (410, 2),
        s if s.parse::<u16>().is_ok() => (s.parse().unwrap(), 2),
        _ => (302, 1), // Default to temporary redirect
    };
    
    if parts.len() <= from_idx {
        return None;
    }
    
    let from = parts[from_idx].to_string();
    
    // "gone" status has no target URL
    let to = if status == 410 {
        None
    } else if parts.len() > from_idx + 1 {
        Some(parts[from_idx + 1].to_string())
    } else {
        return None; // Need a target for non-gone redirects
    };
    
    Some(RedirectRule {
        status,
        from,
        to,
        is_regex,
    })
}
