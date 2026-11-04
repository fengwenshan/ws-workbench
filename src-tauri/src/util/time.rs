//! 时间格式统一出口：落盘一律用带时区的 RFC3339 或毫秒时间戳。

use chrono::Local;

/// 本地时区的 RFC3339 字符串，用于日志与快照展示。
pub fn now_local_rfc3339() -> String {
    Local::now().to_rfc3339()
}

/// UTC 毫秒时间戳，用于附件等需要排序/比较的数值字段。
pub fn now_utc_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
