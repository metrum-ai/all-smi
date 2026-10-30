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
        if in_xrt {
            if let Some((key, value)) = line.split_once(':') {
                let name = match key.trim() {
                    "Version" => Some("xrt_version"),
                    "NPU Firmware Version" => Some("firmware_version"),
                    _ => None,
                };
                insert_value(&mut fields, name, value);
            }
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

/// Refresh once per minute at most, including failures, across all readers.
#[cfg(target_os = "windows")]
pub fn cached_details() -> HashMap<String, String> {
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    type Cache = Option<(Instant, HashMap<String, String>)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let Ok(mut cache) = CACHE.get_or_init(|| Mutex::new(None)).lock() else {
        return HashMap::new();
    };
    if let Some((when, fields)) = cache.as_ref() {
        if when.elapsed() < Duration::from_secs(60) {
            return fields.clone();
        }
    }
    let started = Instant::now();
    let tool = std::path::Path::new(r"C:\Windows\System32\AMD\xrt-smi.exe");
    let run = |args: &[&str]| -> Option<String> {
        let output = Command::new(tool).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).ok())
            .flatten()
    };
    let fields = if tool.is_file() {
        parse_examine(
            &run(&["examine"]).unwrap_or_default(),
            &run(&["examine", "-r", "platform"]).unwrap_or_default(),
        )
    } else {
        HashMap::new()
    };
    *cache = Some((started, fields.clone()));
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
        assert_eq!(fields.get("platform_name").unwrap(), "NPU GorgonPoint1");
        assert_eq!(fields.get("power_mode").unwrap(), "Performance");
        assert_eq!(fields.get("total_columns").unwrap(), "8");
        assert_eq!(fields.len(), 5);
    }

    #[test]
    fn malformed_and_unavailable_values_are_ignored() {
        let fields = parse_examine("Other\n  Version : 99\n", "Name: N/A\nPower Mode:\n");
        assert!(fields.is_empty());
    }
}
