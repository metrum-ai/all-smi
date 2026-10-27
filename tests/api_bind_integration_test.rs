// Copyright (c) 2026 Metrum AI. All rights reserved.

#![cfg(all(unix, feature = "cli"))]

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn forward_log_lines<R>(reader: R, tx: mpsc::UnboundedSender<String>)
where
    R: AsyncBufRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

fn parse_listening_address(line: &str) -> Option<SocketAddr> {
    let address = line.split_once("API server listening on ")?.1;
    address.split_whitespace().next()?.parse().ok()
}

#[tokio::test]
async fn cli_bind_zero_starts_real_api_on_loopback() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_all-smi"))
        .args(["api", "--bind", "127.0.0.1:0", "--interval", "60"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start all-smi API process");

    let stdout = BufReader::new(child.stdout.take().expect("capture stdout"));
    let stderr = BufReader::new(child.stderr.take().expect("capture stderr"));
    let (tx, mut rx) = mpsc::unbounded_channel();
    forward_log_lines(stdout, tx.clone());
    forward_log_lines(stderr, tx.clone());
    drop(tx);

    let address = match timeout(Duration::from_secs(30), async {
        while let Some(line) = rx.recv().await {
            if let Some(address) = parse_listening_address(&line) {
                return address;
            }
        }
        panic!("API process exited before reporting its listener address");
    })
    .await
    {
        Ok(address) => address,
        Err(_) => {
            let _ = child.kill().await;
            panic!("API process did not bind within 30 seconds");
        }
    };

    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);

    let response = timeout(
        Duration::from_secs(10),
        reqwest::get(format!("http://{address}/metrics")),
    )
    .await
    .expect("metrics request timed out")
    .expect("request metrics from live API");
    assert!(response.status().is_success());
    let body = response.text().await.expect("read metrics response");
    assert!(body.contains("all_smi_up{"), "missing API baseline metrics");

    child.kill().await.expect("stop API process");
    let _ = child.wait().await;
}
