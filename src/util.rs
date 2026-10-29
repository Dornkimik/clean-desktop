use std::time::SystemTime;

use chrono::{DateTime, Local};

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_date(time: SystemTime) -> String {
    let dt: DateTime<Local> = time.into();
    dt.format("%d %b %Y, %H:%M").to_string()
}

pub fn age_days(time: SystemTime) -> Option<u64> {
    SystemTime::now()
        .duration_since(time)
        .ok()
        .map(|d| d.as_secs() / 86_400)
}

pub fn relative_time(time: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(time) else {
        return "just now".into();
    };
    let secs = elapsed.as_secs();
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match secs {
        0..60 => "just now".into(),
        60..3_600 => plural(secs / 60, "minute"),
        3_600..86_400 => plural(secs / 3_600, "hour"),
        86_400..604_800 => plural(secs / 86_400, "day"),
        604_800..2_629_800 => plural(secs / 604_800, "week"),
        2_629_800..31_557_600 => plural(secs / 2_629_800, "month"),
        _ => plural(secs / 31_557_600, "year"),
    }
}
