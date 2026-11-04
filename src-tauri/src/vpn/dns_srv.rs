//! fake-ip DNS 应答。
//!
//! 只处理最基础的「单问题」查询：A 查出假地址、AAAA 返回空答案、
//! 其余类型原样空答。上游真实解析在引擎侧按规则走出口完成。

use std::net::Ipv4Addr;

pub struct DnsQuery {
    pub id: u16,
    pub name: String,
    pub qtype: u16,
    /// 问题段在报文中的结束偏移（应答要原样拷回问题段）
    pub question_end: usize,
}

fn read_name(packet: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    loop {
        if pos >= packet.len() {
            return None;
        }
        let len = packet[pos] as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            return None; // 查询里不应出现压缩指针
        }
        pos += 1;
        if pos + len > packet.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&packet[pos..pos + len]).to_string());
        pos += len;
    }
    Some((labels.join("."), pos))
}

pub fn parse_query(packet: &[u8]) -> Option<DnsQuery> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let qdcount = u16::from_be_bytes([packet[4], packet[5]]);
    if qdcount != 1 {
        return None;
    }
    let (name, pos) = read_name(packet, 12)?;
    if pos + 4 > packet.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
    Some(DnsQuery { id, name, qtype, question_end: pos + 4 })
}

fn header_for(query: &[u8], q: &DnsQuery, ancount: u16, rcode: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(q.question_end + 16);
    out.extend_from_slice(&q.id.to_be_bytes());
    let rd = query[2] & 0x01; // 沿用原查询的 RD 位
    out.push(0x80 | rd); // QR=1
    out.push(0x80 | (rcode & 0x0F)); // RA=1
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&ancount.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(&query[12..q.question_end]);
    out
}

pub fn build_a_response(query: &[u8], q: &DnsQuery, ip: Ipv4Addr) -> Vec<u8> {
    let mut out = header_for(query, q, 1, 0);
    out.extend_from_slice(&[0xC0, 0x0C]); // 指向问题里的域名
    out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
    out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    out.extend_from_slice(&60u32.to_be_bytes()); // TTL 60s
    out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    out.extend_from_slice(&ip.octets());
    out
}

pub fn build_empty_response(query: &[u8], q: &DnsQuery) -> Vec<u8> {
    header_for(query, q, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_a(name: &str) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&1u16.to_be_bytes()); // A
        q.extend_from_slice(&1u16.to_be_bytes()); // IN
        q
    }

    #[test]
    fn parses_question_name_and_type() {
        let q = parse_query(&query_a("example.com")).unwrap();
        assert_eq!(q.name, "example.com");
        assert_eq!(q.qtype, 1);
        assert_eq!(q.id, 0x1234);
    }

    #[test]
    fn builds_a_answer_with_fake_ip() {
        let raw = query_a("example.com");
        let q = parse_query(&raw).unwrap();
        let resp = build_a_response(&raw, &q, "198.18.0.5".parse().unwrap());
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1); // ANCOUNT=1
        assert_eq!(&resp[resp.len() - 4..], &[198, 18, 0, 5]);
    }

    #[test]
    fn aaaa_query_returns_empty_answer() {
        let mut raw = query_a("example.com");
        let n = raw.len();
        raw[n - 4] = 0;
        raw[n - 3] = 28; // AAAA
        let q = parse_query(&raw).unwrap();
        assert_eq!(q.qtype, 28);
        let resp = build_empty_response(&raw, &q);
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 0);
    }
}
