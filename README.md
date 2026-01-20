# 🐺 WolfServe

A high-performance web server written in Rust that serves PHP applications via FastCGI with native SSL/TLS support and Apache configuration compatibility.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

## ✨ Features

- **Blazing Fast** - Built on Axum & Tokio for maximum async performance
- **PHP Support** - Execute PHP files via FastCGI (php-fpm or php-cgi)
- **SSL/TLS** - Native HTTPS support with SNI for multiple domains
- **Apache Compatible** - Reads existing Apache vhost configurations
- **Static Files** - Serves static assets efficiently
- **PHP FFI Bridge** - Call Rust functions directly from PHP via libwolflib
- **Admin Dashboard** - Real-time monitoring, statistics, and request logging on port 5000
- **Cross-Platform** - Works on Debian/Ubuntu, Fedora/RHEL, Arch Linux, openSUSE

## 📊 Admin Dashboard

WolfServe includes a built-in admin dashboard accessible on **port 5000** for monitoring and statistics.

### Features

- **Real-time Statistics** - Total requests, response codes (2xx/3xx/4xx/5xx), avg response time, requests/sec
- **Request Logging** - Last 50 requests with method, path, status, duration, client IP, and host
- **Uptime Tracking** - Server uptime displayed in days, hours, minutes, seconds
- **Auto-refresh** - Dashboard updates every 5 seconds
- **Secure Authentication** - Session-based login with bcrypt password hashing

### Default Credentials

- **Username**: `admin`
- **Password**: `admin`

⚠️ **Important**: Change the default password immediately after first login!

### Access

```
http://your-server:5000/
```

The dashboard binds to `0.0.0.0:5000` and is accessible from any interface, including through proxies.

### Password Storage

Credentials are stored in `wolfserve_admin.dat` using base64 encoding with bcrypt password hashing. The file is created automatically on first run.

## 📋 Requirements

- Linux with systemd
- Rust 1.70+ (for building from source)
- PHP 7.4+ with php-fpm

## 🚀 Quick Start

### Option 1: Install from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/wolfserve.git
cd wolfserve

# Run the installer (requires root)
sudo ./install.sh

# Start the server
./run.sh
```

### Option 2: Install Precompiled Binaries

```bash
# Download the release package
tar -xzf wolfserve-x86_64-YYYYMMDD.tar.gz
cd wolfserve-x86_64-YYYYMMDD

# Run the installer (requires root)
sudo ./install_precompiled.sh
```

### Option 3: Install as a Service

```bash
# Build and install as systemd service
sudo ./install_service.sh

# The server will start automatically
systemctl status wolfserve
```

## ⚙️ Configuration

Edit `wolfserve.toml`:

```toml
[server]
host = "0.0.0.0"
port = 3000

[php]
fpm_address = "127.0.0.1:9993"
session_save_path = "/mnt/shared/wolfserve/sessions"

[apache]
# Load existing Apache vhost configs
# Debian/Ubuntu: "/etc/apache2"
# Fedora/RHEL:   "/etc/httpd"
config_dir = "/etc/apache2"
```

## 🌐 Multi-Server PHP Sessions

WolfServe supports shared PHP sessions across multiple servers, enabling seamless load balancing without sticky sessions.

### Configuration

Set `session_save_path` in `wolfserve.toml` to a shared network location:

```toml
[php]
fpm_address = "127.0.0.1:9993"
session_save_path = "/mnt/shared/wolfserve/sessions"
```

### How It Works

1. User logs in on **Server A** → session file created at `/mnt/shared/wolfserve/sessions/sess_abc123`
2. Next request routed to **Server B** → reads the same session file from shared storage
3. User stays logged in seamlessly across all servers

### Requirements

- All servers must mount the same shared storage (NFS, GlusterFS, Ceph, etc.)
- Clocks should be synchronized (NTP) for consistent session expiry
- The installer automatically sets correct permissions (`chmod 1733` with sticky bit)

### Interactive Installation

The service installer prompts for all configuration options:

```bash
sudo ./install_service.sh

📝 Configuration Options (press Enter to accept defaults)

Server bind address [0.0.0.0]: 
Server port [3000]: 
PHP-FPM port [9993]: 
PHP session save path [/var/lib/php/sessions]: /mnt/shared/wolfserve/sessions
Apache config directory [/etc/apache2]: 
```

### Non-Interactive Installation

Use environment variables for automated deployments:

```bash
sudo WOLFSERVE_SESSION_PATH="/mnt/shared/wolfserve/sessions" \
     WOLFSERVE_PORT="3000" \
     ./install_service.sh -y
```

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `WOLFSERVE_HOST` | Server bind address | `0.0.0.0` |
| `WOLFSERVE_PORT` | Server port | `3000` |
| `WOLFSERVE_FPM_PORT` | PHP-FPM port | `9993` |
| `WOLFSERVE_SESSION_PATH` | PHP session save path | `/var/lib/php/sessions` |
| `WOLFSERVE_APACHE_DIR` | Apache config directory | `/etc/apache2` |

## 📁 Project Structure

```
wolfserve/
├── src/
│   ├── main.rs          # Main server code
│   ├── apache.rs        # Apache config parser
│   └── admin.rs         # Admin dashboard & authentication
├── wolflib/             # Rust library for PHP FFI
│   └── src/lib.rs
├── public/              # Web root directory
│   ├── index.php
│   └── rust.php         # PHP FFI example
├── install.sh           # Source installation script
├── install_service.sh   # Systemd service installer
├── install_precompiled.sh # Precompiled binary installer
└── wolfserve.toml       # Server configuration
```

## 🔧 PHP FFI Integration

WolfServe includes `libwolflib.so`, a Rust library that can be called directly from PHP using FFI:

```php
<?php
$ffi = FFI::cdef("
    int wolf_add(int a, int b);
    char* wolf_greet(const char* name);
    void wolf_free_string(char* s);
", "/opt/wolfserve/libwolflib.so");

// Call Rust from PHP!
$result = $ffi->wolf_add(10, 32);  // Returns 42

$greeting = $ffi->wolf_greet("World");
echo FFI::string($greeting);  // "Hello, World from Rust!"
$ffi->wolf_free_string($greeting);
```

## 🐧 Supported Distributions

| Distribution | Package Manager | Status |
|--------------|-----------------|--------|
| Ubuntu/Debian | apt | ✅ Fully Supported |
| Fedora | dnf | ✅ Fully Supported |
| RHEL/CentOS/Rocky | dnf | ✅ Fully Supported |
| Arch Linux | pacman | ✅ Supported |
| openSUSE | zypper | ✅ Supported |

## 📝 Service Management

```bash
# Start the service
sudo systemctl start wolfserve

# Stop the service
sudo systemctl stop wolfserve

# Restart the service
sudo systemctl restart wolfserve

# View logs
sudo journalctl -u wolfserve -f

# Check status
sudo systemctl status wolfserve
```

## ⚠️ Important Notes

- **Apache Conflict**: If Apache is running on ports 80/443, stop and disable it:
  ```bash
  # Debian/Ubuntu
  sudo systemctl stop apache2 && sudo systemctl disable apache2
  
  # Fedora/RHEL
  sudo systemctl stop httpd && sudo systemctl disable httpd
  ```

- **SELinux (Fedora/RHEL)**: The installer automatically configures SELinux permissions.

- **PHP Sessions**: For single-server setups, sessions are stored in `/var/lib/php/sessions`. For multi-server deployments, configure `session_save_path` to point to shared storage (see [Multi-Server PHP Sessions](#-multi-server-php-sessions)).

## 🏗️ Building from Source

```bash
# Build the server
cargo build --release

# Build the PHP FFI library
cd wolflib && cargo build --release

# Or use the build script
./build_lib.sh
```

## 📦 Creating a Release Package

```bash
# Build and package for distribution
./package_release.sh

# Output: release-package/wolfserve-x86_64-YYYYMMDD.tar.gz
```

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details. You are free to use, modify, and distribute this software for both commercial and non-commercial purposes.

## 🙏 Acknowledgments

- [Axum](https://github.com/tokio-rs/axum) - Web framework
- [Tokio](https://tokio.rs/) - Async runtime
- [Rustls](https://github.com/rustls/rustls) - TLS implementation
