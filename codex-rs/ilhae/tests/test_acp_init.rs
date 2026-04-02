use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn test_acp_initialization() {
    let mut child = Command::new("cargo")
        .args(["run", "--bin", "ilhae-proxy", "--", "--experimental-acp"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn proxy");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut stdout = child.stdout.take().expect("Failed to open stdout");

    // Give it a moment to start
    std::thread::sleep(Duration::from_secs(2));

    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
    let init_msg = format!("{}\n", init_msg);

    stdin.write_all(init_msg.as_bytes()).unwrap();
    stdin.flush().unwrap();
    println!("Sent initialize message");

    let mut response = vec![0; 4096];
    match stdout.read(&mut response) {
        Ok(n) if n > 0 => {
            let resp_str = String::from_utf8_lossy(&response[..n]);
            println!("Received response: {}", resp_str);
        }
        Ok(_) => println!("Received EOF"),
        Err(e) => println!("Error reading stdout: {}", e),
    }

    child.kill().unwrap();
}
