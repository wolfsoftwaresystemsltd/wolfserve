use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: php2rust <input.php> [output.rs]");
        return;
    }

    let input_path = &args[1];
    let output_path = if args.len() > 2 {
        args[2].clone()
    } else {
        Path::new(input_path)
            .with_extension("rs")
            .to_string_lossy()
            .into_owned()
    };

    println!("Compiling {} to {}...", input_path, output_path);

    let input_file = File::open(input_path).expect("Could not open input file");
    let reader = BufReader::new(input_file);
    let mut output_file = File::create(output_path).expect("Could not create output file");

    writeln!(output_file, "fn main() {{").unwrap();

    let mut in_php_block = false;

    for line in reader.lines() {
        let line = line.unwrap();
        let trimmed = line.trim();

        if trimmed.starts_with("<?php") {
            in_php_block = true;
            continue;
        }
        if trimmed.starts_with("?>") {
            in_php_block = false;
            continue;
        }

        if in_php_block {
            if trimmed.starts_with("echo") {
                // Handle echo "string";
                let content = trimmed
                    .trim_start_matches("echo")
                    .trim_end_matches(';')
                    .trim();
                writeln!(output_file, "    println!({});", content).unwrap();
            } else if trimmed.starts_with("$") {
                // Handle $var = val;
                // Simple parser: split by =
                if let Some((left, right)) = trimmed.split_once('=') {
                     let var_name = left.trim().trim_start_matches('$');
                     let value = right.trim().trim_end_matches(';');
                     writeln!(output_file, "    let {} = {};", var_name, value).unwrap();
                }
            } else if trimmed.starts_with("//") || trimmed.starts_with("#") {
                 writeln!(output_file, "    {}", trimmed).unwrap();
            }
        } else {
            // HTML content outside PHP tags - logic would be to print it
            if !trimmed.is_empty() {
                writeln!(output_file, "    println!(\"{}\");", line.replace("\"", "\\\"")).unwrap();
            }
        }
    }

    writeln!(output_file, "}}").unwrap();
    println!("Compilation complete.");
}
