use std::path::Path;

use chrono::{Local, TimeZone};

pub(super) fn format_local_time(timestamp: i64, format_str: &str, fallback: &str) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|datetime| datetime.format(format_str).to_string())
        .unwrap_or_else(|| fallback.to_owned())
}

pub(super) fn display_time(detected_at: i64) -> String {
    format_local_time(detected_at, "%m/%d %H:%M", "--/-- --:--")
}

pub(super) fn display_log_time(occurred_at: i64) -> String {
    format_local_time(occurred_at, "%m/%d %H:%M:%S", "--/-- --:--:--")
}

pub(super) fn format_ms(value: u64) -> String {
    if value >= 1000 && value.is_multiple_of(1000) {
        format!("{} 秒", value / 1000)
    } else {
        format!("{value} ms")
    }
}

pub(super) fn looks_like_image(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg"
    )
}
