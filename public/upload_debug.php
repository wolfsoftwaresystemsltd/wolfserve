<?php
/**
 * Upload Debug Script - Check why file uploads aren't working
 */

echo "<h1>PHP Upload Configuration Check</h1>\n";

echo "<h2>1. PHP Upload Settings</h2>\n";
echo "<table border='1'>\n";

$settings = [
    'file_uploads' => 'Must be "1" or "On"',
    'upload_max_filesize' => 'Maximum file size allowed',
    'post_max_size' => 'Must be >= upload_max_filesize',
    'upload_tmp_dir' => 'Temporary directory for uploads',
    'max_file_uploads' => 'Maximum number of files per request',
    'max_input_time' => 'Maximum time to parse input (-1 = unlimited)',
    'memory_limit' => 'PHP memory limit',
];

foreach ($settings as $key => $description) {
    $value = ini_get($key);
    $status = '';
    
    // Check for problems
    if ($key === 'file_uploads' && !$value) {
        $status = '<span style="color:red">⚠ DISABLED!</span>';
    } elseif ($key === 'upload_tmp_dir') {
        if (empty($value)) {
            $value = sys_get_temp_dir() . ' (system default)';
        }
        if (!is_writable($value ?: sys_get_temp_dir())) {
            $status = '<span style="color:red">⚠ NOT WRITABLE!</span>';
        } else {
            $status = '<span style="color:green">✓ Writable</span>';
        }
    }
    
    echo "<tr><td><b>$key</b></td><td>$value</td><td>$status</td><td>$description</td></tr>\n";
}
echo "</table>\n";

echo "<h2>2. Temp Directory Check</h2>\n";
$tmpDir = ini_get('upload_tmp_dir') ?: sys_get_temp_dir();
echo "Upload temp directory: <b>$tmpDir</b><br>\n";
echo "Exists: " . (is_dir($tmpDir) ? '<span style="color:green">Yes</span>' : '<span style="color:red">No</span>') . "<br>\n";
echo "Writable: " . (is_writable($tmpDir) ? '<span style="color:green">Yes</span>' : '<span style="color:red">No</span>') . "<br>\n";

echo "<h2>3. Request Information</h2>\n";
echo "REQUEST_METHOD: " . ($_SERVER['REQUEST_METHOD'] ?? 'not set') . "<br>\n";
echo "CONTENT_TYPE: " . ($_SERVER['CONTENT_TYPE'] ?? 'not set') . "<br>\n";
echo "CONTENT_LENGTH: " . ($_SERVER['CONTENT_LENGTH'] ?? 'not set') . "<br>\n";
echo "HTTP_CONTENT_TYPE: " . ($_SERVER['HTTP_CONTENT_TYPE'] ?? 'not set') . "<br>\n";

echo "<h2>4. Upload Test Results (POST only)</h2>\n";
if ($_SERVER['REQUEST_METHOD'] === 'POST') {
    echo "<h3>\$_FILES:</h3>\n";
    echo "<pre>" . print_r($_FILES, true) . "</pre>\n";
    
    echo "<h3>\$_POST:</h3>\n";
    echo "<pre>" . print_r($_POST, true) . "</pre>\n";
    
    if (!empty($_FILES)) {
        foreach ($_FILES as $name => $file) {
            echo "<h4>File: $name</h4>\n";
            if ($file['error'] !== UPLOAD_ERR_OK) {
                $errors = [
                    UPLOAD_ERR_INI_SIZE => 'File exceeds upload_max_filesize',
                    UPLOAD_ERR_FORM_SIZE => 'File exceeds MAX_FILE_SIZE in form',
                    UPLOAD_ERR_PARTIAL => 'File was only partially uploaded',
                    UPLOAD_ERR_NO_FILE => 'No file was uploaded',
                    UPLOAD_ERR_NO_TMP_DIR => 'Missing a temporary folder',
                    UPLOAD_ERR_CANT_WRITE => 'Failed to write file to disk',
                    UPLOAD_ERR_EXTENSION => 'A PHP extension stopped the file upload',
                ];
                $errMsg = $errors[$file['error']] ?? 'Unknown error ' . $file['error'];
                echo "<span style='color:red'>Error: $errMsg</span><br>\n";
            } else {
                echo "<span style='color:green'>Upload successful!</span><br>\n";
                echo "Temp file: {$file['tmp_name']}<br>\n";
                echo "Size: {$file['size']} bytes<br>\n";
            }
        }
    } else {
        echo "<span style='color:orange'>No files in \$_FILES - check if multipart form data is being passed correctly</span><br>\n";
    }
    
    // Check raw input
    echo "<h3>Raw php://input (first 500 bytes):</h3>\n";
    $raw = file_get_contents('php://input');
    echo "Length: " . strlen($raw) . " bytes<br>\n";
    echo "<pre>" . htmlspecialchars(substr($raw, 0, 500)) . "</pre>\n";
} else {
    echo "<p>Submit the form below to test uploads:</p>\n";
}

echo "<h2>5. Test Upload Form</h2>\n";
?>
<form method="POST" enctype="multipart/form-data">
    <input type="file" name="testfile"><br><br>
    <input type="text" name="testfield" value="test value"><br><br>
    <input type="submit" value="Test Upload">
</form>

<h2>6. All $_SERVER Variables</h2>
<details>
<summary>Click to expand</summary>
<pre><?php print_r($_SERVER); ?></pre>
</details>
