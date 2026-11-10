//! 极简 DNS 客户端：构造 A 查询、从应答里取回 A 记录。
//!
//! 分流需要在用户态主动查一次 VPN DNS（Windows 回退路径的后台解析、按域名
//! 自检都走这里）。不能依赖系统解析器：NRPT 会把查询交回我们自己的转发器，
//! 再走 `getaddrinfo` 就绕成递归。这里只实现最小闭环：发一个 A 查询、解析应答。

use std::net::Ipv4Addr;

/// 构造一个 A 查询（单问题、RD=1）。
pub fn build_query(name: &str, id: u16) -> Option<Vec<u8>> {
    let name = name.trim().trim_end_matches('.');
    if name.is_empty() || name.len() > 253 {
        return None;
    }
    let mut out = Vec::with_capacity(32 + name.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
    out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    Some(out)
}

/// 跳过（可能带压缩指针的）域名，返回其后的偏移。
fn skip_name(packet: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos];
        match len {
            0 => return Some(pos + 1),
            // 压缩指针固定两字节，指向哪里都不影响「跳过」
            0xC0..=0xFF => return Some(pos + 2),
            _ => {
                pos += 1 + len as usize;
                if pos > packet.len() {
                    return None;
                }
            }
        }
    }
}

/// 读第一个问题的名字（小写、去尾点）。
///
/// 转发器要靠它判断「这条查询属于哪个域名」，再决定要不要按应答注入 `/32`：
/// 直连例外（`DOMAIN hlwapi.example.com → 直连`）的查询也会进到转发器
/// （它在父级后缀的 resolver/NRPT 范围里），但它的地址不能被塞进隧道。
///
/// 问题段按 RFC 1035 不允许压缩指针，遇到就返回 `None`（调用方按旧行为处理）。
pub fn parse_question_name(packet: &[u8]) -> Option<String> {
    if packet.len() < 12 {
        return None;
    }
    let mut labels: Vec<String> = Vec::new();
    let mut pos = 12usize;
    loop {
        let len = *packet.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let label = packet.get(pos..pos + len)?;
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        pos += len;
        if labels.len() > 128 {
            return None;
        }
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join("."))
    }
}

/// 判断一个报文是不是指定查询 ID 的应答，避免把迟到的旧应答当成结果。
pub fn is_response_to(packet: &[u8], id: u16) -> bool {
    packet.len() >= 12
        && u16::from_be_bytes([packet[0], packet[1]]) == id
        && packet[2] & 0x80 != 0
}

/// 从 DNS 应答里收集所有 A 记录（CNAME 链后面的也会被收进来）。
pub fn parse_a_records(packet: &[u8]) -> Vec<Ipv4Addr> {
    if packet.len() < 12 {
        return Vec::new();
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 == 0 {
        return Vec::new(); // 不是应答
    }
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let ancount = u16::from_be_bytes([packet[6], packet[7]]) as usize;

    let mut pos = 12usize;
    for _ in 0..qdcount {
        pos = match skip_name(packet, pos) {
            Some(p) => p + 4, // TYPE + CLASS
            None => return Vec::new(),
        };
    }

    let mut out = Vec::new();
    for _ in 0..ancount {
        pos = match skip_name(packet, pos) {
            Some(p) => p,
            None => break,
        };
        if pos + 10 > packet.len() {
            break;
        }
        let rtype = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let class = u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]);
        let rdlen = u16::from_be_bytes([packet[pos + 8], packet[pos + 9]]) as usize;
        let rd_start = pos + 10;
        if rd_start + rdlen > packet.len() {
            break;
        }
        if rtype == 1 && class == 1 && rdlen == 4 {
            out.push(Ipv4Addr::new(
                packet[rd_start],
                packet[rd_start + 1],
                packet[rd_start + 2],
                packet[rd_start + 3],
            ));
        }
        pos = rd_start + rdlen;
    }
    out
}

/// 应答里的最小 TTL（秒）；没有答案时返回 `None`。
pub fn min_ttl(packet: &[u8]) -> Option<u32> {
    if packet.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let ancount = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut pos = 12usize;
    for _ in 0..qdcount {
        pos = skip_name(packet, pos)? + 4;
    }
    let mut ttl: Option<u32> = None;
    for _ in 0..ancount {
        pos = skip_name(packet, pos)?;
        if pos + 10 > packet.len() {
            return ttl;
        }
        let current = u32::from_be_bytes([
            packet[pos + 4],
            packet[pos + 5],
            packet[pos + 6],
            packet[pos + 7],
        ]);
        ttl = Some(ttl.map_or(current, |old| old.min(current)));
        let rdlen = u16::from_be_bytes([packet[pos + 8], packet[pos + 9]]) as usize;
        pos += 10 + rdlen;
    }
    ttl
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(name: &str, ip: Ipv4Addr) -> Vec<u8> {
        let query = build_query(name, 0x1234).unwrap();
        let mut out = query.clone();
        // 头部改成应答
        out[2] = 0x81;
        out[3] = 0x80;
        out[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT
        // answer: 指针 + A + IN + TTL + RDLEN + IP
        out.extend_from_slice(&[0xC0, 0x0C]);
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&60u32.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&ip.octets());
        out
    }

    #[test]
    fn builds_a_valid_query() {
        let q = build_query("git.example.com", 0x1234).unwrap();
        assert_eq!(u16::from_be_bytes([q[0], q[1]]), 0x1234);
        assert_eq!(u16::from_be_bytes([q[4], q[5]]), 1);
        assert_eq!(q[q.len() - 4], 0); // 根标签
        assert_eq!(&q[q.len() - 4..], &[0, 1, 0, 1]);
    }

    #[test]
    fn rejects_invalid_names() {
        assert!(build_query("", 1).is_none());
        assert!(build_query("a..b", 1).is_none());
        assert!(build_query(&"a".repeat(64), 1).is_none());
    }

    #[test]
    fn reads_question_name_from_query_and_answer() {
        let query = build_query("HLWAPI.example.com.", 7).unwrap();
        assert_eq!(
            parse_question_name(&query).as_deref(),
            Some("hlwapi.example.com")
        );
        let answer = response("oa.example.com", Ipv4Addr::new(10, 10, 3, 4));
        assert_eq!(
            parse_question_name(&answer).as_deref(),
            Some("oa.example.com")
        );
    }

    #[test]
    fn question_name_is_none_for_junk() {
        assert!(parse_question_name(&[]).is_none());
        assert!(parse_question_name(&[0u8; 12]).is_none());
        // 压缩指针出现在问题段：按约定不解析
        let mut packet = build_query("a.com", 1).unwrap();
        packet[12] = 0xC0;
        assert!(parse_question_name(&packet).is_none());
    }

    #[test]
    fn parses_a_record() {
        let packet = response("git.example.com", Ipv4Addr::new(10, 1, 2, 3));
        assert_eq!(
            parse_a_records(&packet),
            vec![Ipv4Addr::new(10, 1, 2, 3)]
        );
        assert_eq!(min_ttl(&packet), Some(60));
    }

    #[test]
    fn ignores_queries_and_non_a_answers() {
        let query = build_query("a.com", 1).unwrap();
        assert!(parse_a_records(&query).is_empty());

        let mut packet = response("a.com", Ipv4Addr::new(1, 2, 3, 4));
        // 把 answer 的 TYPE 改成 AAAA
        let answer_type = packet.len() - 14;
        packet[answer_type..answer_type + 2].copy_from_slice(&28u16.to_be_bytes());
        assert!(parse_a_records(&packet).is_empty());
    }
}
