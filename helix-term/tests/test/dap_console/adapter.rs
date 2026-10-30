//! A stdio adapter running in the integration-test executable. File gates let
//! tests hold a DAP reply while exercising the editor, without network access or
//! an installed language debugger.

use std::{
    collections::HashSet,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use helix_core::syntax::config::DebugAdapterConfig;
use serde_json::{json, Value};

pub(super) fn config(gates: &Path) -> anyhow::Result<DebugAdapterConfig> {
    Ok(DebugAdapterConfig {
        name: "console-test".into(),
        transport: "stdio".into(),
        command: std::env::current_exe()?.to_string_lossy().into_owned(),
        args: vec![
            "--exact".into(),
            "test::dap_console::adapter::serve".into(),
            "--ignored".into(),
            "--nocapture".into(),
            // A test-harness filter carries the gate directory to the helper
            // without changing process-wide environment variables.
            format!("--skip={}", gates.display()),
        ],
        port_arg: None,
        templates: Vec::new(),
        quirks: Default::default(),
    })
}

pub(super) async fn wait_for(path: PathBuf) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

fn wait_for_release(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "adapter gate timed out: {path:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "stdio adapter helper, started by console integration tests"]
fn serve() -> anyhow::Result<()> {
    let gates = std::env::args()
        .find_map(|arg| arg.strip_prefix("--skip=").map(PathBuf::from))
        .expect("adapter gate directory");
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut sequences = HashSet::new();
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                return Ok(());
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
        let mut bytes = vec![0; length.expect("DAP content length")];
        input.read_exact(&mut bytes)?;
        let request: Value = serde_json::from_slice(&bytes)?;
        assert!(
            sequences.insert(request["seq"].as_u64().unwrap()),
            "duplicate request sequence"
        );
        let command = request["command"].as_str().unwrap();
        let body = match command {
            "initialize" => json!({ "supportsCompletionsRequest": true }),
            "evaluate" => {
                let expression = request["arguments"]["expression"].as_str().unwrap();
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(gates.join("evaluated"))?;
                writeln!(log, "{expression}:{}", request["arguments"]["frameId"])?;
                if expression == "slow" {
                    std::fs::write(gates.join("evaluate-started"), "")?;
                    wait_for_release(&gates.join("release-evaluate"));
                }
                json!({ "result": if expression == "slow" { "11" } else { "22" }, "variablesReference": 0 })
            }
            "completions" => {
                std::fs::write(gates.join("completion-started"), "")?;
                wait_for_release(&gates.join("release-completion"));
                json!({ "targets": [{ "label": "member", "start": 4, "length": 0 }] })
            }
            "scopes" => {
                std::fs::write(gates.join("inspection-started"), "")?;
                wait_for_release(&gates.join("release-inspection"));
                json!({ "scopes": [{ "name": "Locals", "variablesReference": 100, "expensive": false }] })
            }
            "variables" => {
                json!({ "variables": [{ "name": "answer", "value": "42", "type": "int", "variablesReference": 0 }] })
            }
            _ => json!({}),
        };
        let response = serde_json::to_vec(&json!({
            "type": "response", "request_seq": request["seq"],
            "success": true, "command": command, "body": body,
        }))?;
        write!(output, "Content-Length: {}\r\n\r\n", response.len())?;
        output.write_all(&response)?;
        output.flush()?;
        if command == "disconnect" {
            return Ok(());
        }
    }
}
