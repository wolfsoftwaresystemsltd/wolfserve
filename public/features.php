<?php
session_start();

if (!isset($_SESSION['count'])) {
    $_SESSION['count'] = 0;
}
$_SESSION['count']++;

header('Content-Type: text/html');
?>
<!DOCTYPE html>
<html>
<body>
    <h2>Feature Check</h2>
    
    <h3>Session Test</h3>
    <p>Session Count: <strong><?php echo $_SESSION['count']; ?></strong> (Reload to increase)</p>
    <p>Session ID: <?php echo session_id(); ?></p>

    <h3>Database Support</h3>
    <ul>
        <li>PDO Installed: <?php echo class_exists('PDO') ? '✅ Yes' : '❌ No'; ?></li>
        <li>MySQL Driver: <?php echo extension_loaded('pdo_mysql') || extension_loaded('mysqli') ? '✅ Yes' : '❌ No'; ?></li>
        <li>SQLite Driver: <?php echo extension_loaded('pdo_sqlite') || extension_loaded('sqlite3') ? '✅ Yes' : '❌ No'; ?></li>
    </ul>

    <h3>Environment</h3>
    <p>Server Software: <?php echo $_SERVER['SERVER_SOFTWARE']; ?></p>
</body>
</html>
