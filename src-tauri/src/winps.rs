//! Windows PowerShell 桥接
//!
//! 全项目统一从 PowerShell 取**结构化 JSON**，而不是解析 `netsh` / `route print`
//! 的文本输出。原因是中文系统的命令行输出是本地化的，按文本匹配会随机器变化而失效；
//! CIM 对象的属性名则始终是英文。
//!
//! 强制 `[Console]::OutputEncoding` 为 UTF-8，避免中文网卡名、中文路径被 OEM 代码页破坏。

use serde::Deserialize;
use serde_json::Value;

use crate::process::run_with_timeout;

/// PowerShell 脚本默认执行超时
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// 统一包装：设置输出编码、遇到错误即停止
pub fn wrap(script: &str) -> String {
    format!(
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $ErrorActionPreference='Stop'; {}",
        script
    )
}

fn powershell_path() -> String {
    std::env::var("SystemRoot")
        .map(|root| format!(r"{}\System32\WindowsPowerShell\v1.0\powershell.exe", root))
        .unwrap_or_else(|_| "powershell.exe".to_string())
}

/// 执行一段 PowerShell 脚本，返回标准输出
#[cfg(target_os = "windows")]
pub fn run(script: &str) -> Result<String, String> {
    run_with_timeout_ms(script, DEFAULT_TIMEOUT)
}

/// 带自定义超时地执行
#[cfg(target_os = "windows")]
pub fn run_with_timeout_ms(script: &str, timeout: std::time::Duration) -> Result<String, String> {
    let wrapped = wrap(script);
    let (text, ok) = run_with_timeout(
        std::path::Path::new(&powershell_path()),
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &wrapped,
        ],
        timeout,
    );
    if ok {
        Ok(text)
    } else {
        Err(format!("PowerShell 执行失败: {}", text.trim()))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run(_script: &str) -> Result<String, String> {
    Err("当前平台没有 PowerShell".to_string())
}

#[cfg(target_os = "windows")]
pub fn run_json(script: &str) -> Result<Value, String> {
    let text = run(script)?;
    parse_json(&text)
}

#[cfg(not(target_os = "windows"))]
pub fn run_json(_script: &str) -> Result<Value, String> {
    Err("当前平台没有 PowerShell".to_string())
}

/// 解析 PowerShell 输出的 JSON，顺带处理 BOM 与空输出
pub fn parse_json(text: &str) -> Result<Value, String> {
    let cleaned = text.trim_start_matches('\u{feff}').trim();
    if cleaned.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(cleaned).map_err(|e| {
        format!(
            "解析 JSON 失败: {}（原始输出: {}）",
            e,
            cleaned.chars().take(200).collect::<String>()
        )
    })
}

/// `ConvertTo-Json` 在只有一条记录时返回对象而不是数组，这里统一成数组
pub fn json_array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

/// 把 JSON 里的 `null` 也当作默认值处理。
///
/// `#[serde(default)]` 只能兜住「字段缺失」，兜不住**显式 null**。
/// 而 PowerShell 的 `ConvertTo-Json` 恰恰会为注册表里不存在的值输出 `null`
/// （例如某条卸载记录有 InstallLocation 但没有 DisplayIcon）。
/// 不加这层，这些记录会在反序列化时整个被丢掉，
/// 表现就是「明明装了却找不到」。
pub fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = Option::<T>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

/// 按 schema 把 JSON 数组解析成结构体列表，遇到坏记录直接跳过
pub fn parse_list<T: serde::de::DeserializeOwned>(value: Value) -> Vec<T> {
    json_array(value)
        .into_iter()
        .filter_map(|item| serde_json::from_value::<T>(item).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_object_as_one_element_array() {
        let value: Value = serde_json::from_str(r#"{"a":1}"#).unwrap();
        assert_eq!(json_array(value).len(), 1);
    }

    #[test]
    fn parses_real_array() {
        let value: Value = serde_json::from_str(r#"[{"a":1},{"a":2}]"#).unwrap();
        assert_eq!(json_array(value).len(), 2);
    }

    #[test]
    fn null_becomes_empty_array() {
        assert!(json_array(Value::Null).is_empty());
    }

    #[test]
    fn parse_json_strips_bom_and_blank() {
        assert_eq!(parse_json("\u{feff}{}").unwrap(), serde_json::json!({}));
        assert_eq!(parse_json("   ").unwrap(), Value::Null);
        assert!(parse_json("{not json").is_err());
    }

    #[test]
    fn parse_list_skips_bad_records() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Item {
            #[serde(rename = "a", default)]
            a: i32,
        }
        let value: Value = serde_json::from_str(r#"[{"a":1},{"b":2},{"a":3}]"#).unwrap();
        let items: Vec<Item> = parse_list(value);
        // {"b":2} 缺字段但有 default，所以三条都在
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].a, 0);
    }

    #[derive(serde::Deserialize, Debug, Default, PartialEq)]
    struct TolerantItem {
        #[serde(
            rename = "DisplayName",
            default,
            deserialize_with = "null_as_default"
        )]
        display_name: String,
        #[serde(
            rename = "DisplayIcon",
            default,
            deserialize_with = "null_as_default"
        )]
        display_icon: String,
    }

    #[test]
    fn explicit_null_is_treated_as_default() {
        // 复刻真实注册表：同一条记录有 InstallLocation 却没有 DisplayIcon，
        // 反之亦然。不处理 null 的话这两条都会被丢掉。
        let value: Value = serde_json::from_str(
            r#"[
                {"DisplayName":"Cisco AnyConnect","DisplayIcon":null},
                {"DisplayName":"Cisco AnyConnect","DisplayIcon":"C:\\a\\vpncli.exe"}
            ]"#,
        )
        .unwrap();
        let items: Vec<TolerantItem> = parse_list(value);
        assert_eq!(items.len(), 2, "带 null 的记录不能被丢掉");
        assert_eq!(items[0].display_name, "Cisco AnyConnect");
        assert_eq!(items[0].display_icon, "");
    }

    #[test]
    fn missing_and_null_are_both_tolerated() {
        let value: Value = serde_json::from_str(r#"[{"DisplayName":"X"}]"#).unwrap();
        let items: Vec<TolerantItem> = parse_list(value);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_icon, "");
    }

    #[test]
    fn wrap_sets_utf8_output_encoding() {
        let wrapped = wrap("Get-Date");
        assert!(wrapped.contains("OutputEncoding"));
        assert!(wrapped.contains("UTF8"));
        assert!(wrapped.ends_with("Get-Date"));
    }
}
