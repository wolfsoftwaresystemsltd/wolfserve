<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WolfServe - It Works!</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background-color: #f0f2f5;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            color: #333;
        }
        .container {
            background: white;
            padding: 2rem 3rem;
            border-radius: 8px;
            box-shadow: 0 4px 6px rgba(0,0,0,0.1);
            text-align: center;
            max-width: 500px;
        }
        h1 { color: #d32f2f; margin-bottom: 0.5rem; }
        p { line-height: 1.6; }
        .php-ver {
            background: #eee;
            padding: 0.2rem 0.5rem;
            border-radius: 4px;
            font-family: monospace;
            font-weight: bold;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>WolfServe</h1>
        <p>Your Rust-powered PHP server is running successfully.</p>
        <p>PHP Version: <span class="php-ver"><?php echo phpversion(); ?></span></p>
        <p><?php echo "Hello from dynamic PHP!"; ?></p>
    </div>
</body>
</html>
