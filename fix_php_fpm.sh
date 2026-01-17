#!/bin/bash
# Fix PHP-FPM configuration for file uploads
# This script fixes common PHP-FPM settings that break file uploads

echo "Fixing PHP-FPM configuration..."

# Find all PHP versions and fix their FPM configs
for version in 8.3 8.2 8.1 8.0 7.4; do
    ini_file="/etc/php/$version/fpm/php.ini"
    if [ -f "$ini_file" ]; then
        echo "Fixing $ini_file..."
        
        # Fix post_max_size if it's 0
        if grep -q "^post_max_size = 0" "$ini_file"; then
            sed -i 's/^post_max_size = 0/post_max_size = 520M/' "$ini_file"
            echo "  - Fixed post_max_size (was 0)"
        fi
        
        # Ensure upload settings are reasonable
        sed -i 's/^upload_max_filesize = 2M/upload_max_filesize = 256M/' "$ini_file"
        sed -i 's/^post_max_size = 8M/post_max_size = 520M/' "$ini_file"
        
        # Ensure file_uploads is on
        sed -i 's/^file_uploads = Off/file_uploads = On/' "$ini_file"
        
        # Restart the corresponding PHP-FPM service
        if systemctl is-active --quiet "php$version-fpm"; then
            echo "  - Restarting php$version-fpm..."
            systemctl restart "php$version-fpm"
        fi
    fi
done

# Also fix CLI and phpdbg configs to keep them consistent
for version in 8.3 8.2 8.1 8.0 7.4; do
    for sapi in cli phpdbg cgi; do
        ini_file="/etc/php/$version/$sapi/php.ini"
        if [ -f "$ini_file" ]; then
            if grep -q "^post_max_size = 0" "$ini_file"; then
                sed -i 's/^post_max_size = 0/post_max_size = 520M/' "$ini_file"
                echo "Fixed $ini_file"
            fi
        fi
    done
done

echo ""
echo "PHP-FPM configuration fixed!"
echo ""
echo "Current settings:"
for version in 8.3 8.2 8.1 8.0 7.4; do
    ini_file="/etc/php/$version/fpm/php.ini"
    if [ -f "$ini_file" ]; then
        echo "PHP $version FPM:"
        grep -E "^(post_max_size|upload_max_filesize|file_uploads)" "$ini_file" | sed 's/^/  /'
    fi
done
