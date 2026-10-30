// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Optional AMD XRT identity. This is never a prerequisite for NPU discovery.

use std::collections::HashMap;

/// Parse the documented fields from the two independent examine reports.
/// Unrecognized, empty, and explicitly unavailable values are omitted.
pub fn parse_examine(examine: &str, platform: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let mut in_xrt = false;
    for line in examine.lines() {
        if line.trim() == "XRT" {
            in_xrt = true;
            continue;
        }
        if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            in_xrt = false;
        }
        if in_xrt && let Some((key, value)) = line.split_once(':') {
            let name = match key.trim() {
                "Version" => Some("xrt_version"),
                "NPU Firmware Version" => Some("firmware_version"),
                "NPU Driver Version" => Some("npu_driver_version"),
                _ => None,
            };
            insert_value(&mut fields, name, value);
        }
    }
    for line in platform.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let name = match key.trim() {
                "Name" => Some("platform_name"),
                "Power Mode" => Some("power_mode"),
                "Total Columns" => Some("total_columns"),
                _ => None,
            };
            insert_value(&mut fields, name, value);
        }
    }
    fields
}

fn insert_value(fields: &mut HashMap<String, String>, name: Option<&str>, value: &str) {
    let value = value.trim();
    if let Some(name) = name.filter(|_| !value.is_empty() && !value.eq_ignore_ascii_case("N/A")) {
        fields.insert(name.to_owned(), value.to_owned());
    }
}

/// Run an optional tool without letting a hung child or inherited stdout pipe
/// block a reader indefinitely. Drain stdout concurrently to avoid pipe-buffer
/// deadlocks; bound both output size and the overall wait.
fn run_with_timeout(
    program: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    const MAX_OUTPUT: u64 = 1024 * 1024;
    let started = Instant::now();
    let mut child = crate::utils::command::new_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .ok()
            .filter(|_| bytes.len() as u64 <= MAX_OUTPUT)
            .and_then(|_| String::from_utf8(bytes).ok());
        let _ = sender.send(result);
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                return receiver
                    .recv_timeout(timeout.saturating_sub(started.elapsed()))
                    .ok()
                    .flatten();
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(
                    std::time::Duration::from_millis(25)
                        .min(timeout.saturating_sub(started.elapsed())),
                );
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn xrt_tool_path(system_root: Option<std::ffi::OsString>) -> std::path::PathBuf {
    let root = system_root
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| r"C:\Windows".into());
    std::path::PathBuf::from(root)
        .join("System32")
        .join("AMD")
        .join("xrt-smi.exe")
}

/// Refresh once per minute at most, including failures, across all readers.
#[cfg(target_os = "windows")]
pub fn cached_details() -> HashMap<String, String> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    type Cache = Option<(Instant, HashMap<String, String>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(None)).lock() else {
        return HashMap::new();
    };
    if let Some((when, fields)) = cache.as_ref()
        && when.elapsed() < Duration::from_secs(60)
    {
        return fields.clone();
    }
    let tool = xrt_tool_path(std::env::var_os("SystemRoot"));
    let run = |args: &[&str]| run_with_timeout(&tool, args, Duration::from_secs(5));
    let fields = if tool.is_file() {
        parse_examine(
            &run(&["examine"]).unwrap_or_default(),
            &run(&["examine", "-r", "platform"]).unwrap_or_default(),
        )
    } else {
        HashMap::new()
    };
    *cache = Some((Instant::now(), fields.clone()));
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_laptop_reports_and_ignores_unavailable_values() {
        let examine = "XRT\n  Version : 2.21.0\n  NPU Driver Version : 32.0.20102.3930\n  NPU Firmware Version : 1.1.2.64\nDevice(s) Present\n|[00c5:00:01.1] |NPU GorgonPoint1 |\n";
        let platform = "Name : NPU GorgonPoint1\nPower Mode : Performance\nTotal Columns : 8\nEstimated Power : N/A\n";
        let fields = parse_examine(examine, platform);
        assert_eq!(fields.get("xrt_version").unwrap(), "2.21.0");
        assert_eq!(fields.get("firmware_version").unwrap(), "1.1.2.64");
        assert_eq!(fields.get("npu_driver_version").unwrap(), "32.0.20102.3930");
        assert_eq!(fields.get("platform_name").unwrap(), "NPU GorgonPoint1");
        assert_eq!(fields.get("power_mode").unwrap(), "Performance");
        assert_eq!(fields.get("total_columns").unwrap(), "8");
        assert_eq!(fields.len(), 6);
    }

    #[test]
    fn system_root_path_is_used_and_empty_root_falls_back() {
        let expected = std::path::Path::new("custom-root")
            .join("System32")
            .join("AMD")
            .join("xrt-smi.exe");
        assert_eq!(xrt_tool_path(Some("custom-root".into())), expected);
        assert_eq!(xrt_tool_path(Some("".into())), xrt_tool_path(None));
    }

    #[cfg(unix)]
    #[test]
    fn command_times_out_and_handles_failure_and_success() {
        let shell = std::path::Path::new("/bin/sh");
        let started = std::time::Instant::now();
        assert_eq!(
            run_with_timeout(
                shell,
                &["-c", "exec sleep 5"],
                std::time::Duration::from_millis(100)
            ),
            None
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(
            run_with_timeout(shell, &["-c", "exit 7"], std::time::Duration::from_secs(1)),
            None
        );
        assert_eq!(
            run_with_timeout(
                shell,
                &["-c", "printf ok"],
                std::time::Duration::from_secs(1)
            )
            .as_deref(),
            Some("ok")
        );
    }

    #[test]
    fn malformed_and_unavailable_values_are_ignored() {
        let fields = parse_examine("Other\n  Version : 99\n", "Name: N/A\nPower Mode:\n");
        assert!(fields.is_empty());
    }
}
