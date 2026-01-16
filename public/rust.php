<?php
// PHP FFI Example invoking WolfLib
$libPath = '/home/paulc/NetBeansProjects/wolfserve/wolflib/target/release/libwolflib.so';

header('Content-Type: text/html');

if (!extension_loaded('ffi')) {
    echo "<h1>Error: PHP FFI Extension not loaded</h1>";
    echo "<p>Please enable FFI in your php.ini configuration.</p>";
    exit;
}

try {
    // Define the C signature matching our Rust library
    $ffi = FFI::cdef("
        int wolf_add(int a, int b);
        char* wolf_greet(const char* name);
        void wolf_free_string(char* s);
    ", $libPath);

    $sum = $ffi->wolf_add(10, 32);
    
    // String handling
    $name = "PHP User";
    $c_greeting = $ffi->wolf_greet($name); // Returns *mut i8 (char*)
    $greeting = FFI::string($c_greeting);   // Convert C string to PHP string
    
    // Important: Free the memory allocated by Rust (CString::into_raw)
    $ffi->wolf_free_string($c_greeting);

} catch (Exception $e) {
    echo "<h1>FFI Error</h1>";
    echo "<p>" . $e->getMessage() . "</p>";
    exit;
}
?>
<!DOCTYPE html>
<html>
<head><title>Rust FFI Test</title></head>
<body>
    <h1>Calling Rust from PHP</h1>
    
    <div style="border:1px solid #ccc; padding: 20px; border-radius: 8px; margin: 20px;">
        <h3>Math</h3>
        <p>10 + 32 = <strong><?php echo $sum; ?></strong></p>
    </div>

    <div style="border:1px solid #ccc; padding: 20px; border-radius: 8px; margin: 20px;">
        <h3>String Manipulation</h3>
        <p>Rust says: <em><?php echo htmlspecialchars($greeting); ?></em></p>
    </div>
    
    <p><small>Library loaded from: <?php echo $libPath; ?></small></p>
</body>
</html>
