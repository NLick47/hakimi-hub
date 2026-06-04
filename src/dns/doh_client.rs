// DoH 客户端
//
// 连接池用 LRU 索引做 O(1) 淘汰，不用遍历
// 相同 host 的端点共享连接，省掉重复 TLS 握手

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, trace, warn};

use crate::core::config::DohEndpoint;

use dashmap::DashMap;
use lru::LruCache;

const DOH_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTION_POOL_TTL: Duration = Duration::from_secs(60);
const MAX_POOL_SIZE: usize = 50;

// 连接池里的 TLS 连接
struct PoolEntry {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    last_used: Instant,
}

// DoH 解析结果
pub struct DohResolveResult {
    // 解到的 IP 列表
    pub ips: Vec<IpAddr>,
    // 用的 DoH 服务器（如 ["alidns", "tencent-doh"]）
    pub doh_servers: Vec<String>,
}

// DoH 客户端
// 用 LRU 索引做 O(1) 连接池淘汰
pub struct DohClient {
    endpoints: Vec<DohEndpoint>,
    dns_mapping: HashMap<String, String>,
    preresolved_ips: DashMap<String, Vec<IpAddr>>,
    tls_config: Arc<rustls::ClientConfig>,
    connection_pool: DashMap<String, PoolEntry>,
    // LRU 索引，用于 O(1) 淘汰最久未用的连接
    lru_index: RwLock<LruCache<String, ()>>,
}

impl DohClient {
    pub fn new(
        endpoints: Vec<DohEndpoint>,
        dns_mapping: HashMap<String, String>,
    ) -> Self {
        let tls_config = Arc::new(Self::build_tls_config());

        Self {
            endpoints,
            dns_mapping,
            preresolved_ips: DashMap::new(),
            tls_config,
            connection_pool: DashMap::new(),
            lru_index: RwLock::new(LruCache::unbounded()),
        }
    }

    fn evict_oldest_connection(&self) {
        // 先从 LRU 索引弹出 key（持锁时间最短）
        let old_key = {
            if let Ok(mut lru) = self.lru_index.write() {
                lru.pop_lru().map(|(k, _)| k)
            } else {
                return;
            }
        };

        if let Some(key) = old_key {
            // 再从连接池删除（不持 LRU 锁）
            if self.connection_pool.remove(&key).is_some() {
                debug!("DoH 连接池 O(1) LRU 淘汰: {}", key);
            } else {
                debug!("DoH LRU 索引不同步: {} 不在连接池中", key);
            }
        }
    }

    fn acquire_connection(&self, host: &str) -> Option<tokio_rustls::client::TlsStream<TcpStream>> {
        let key = format!("{}:443", host);

        // 直接移除，拿到后再检查是否过期
        let (_, entry) = self.connection_pool.remove(&key)?;

        if entry.last_used.elapsed() >= CONNECTION_POOL_TTL {
            if let Ok(mut lru) = self.lru_index.write() {
                lru.pop(&key);
            }
            debug!("DoH 连接已过期，删除: {}", key);
            return None;
        }

        if let Ok(mut lru) = self.lru_index.write() {
            lru.pop(&key);
        }
        trace!("复用 DoH 连接: {}", key);
        Some(entry.stream)
    }

    fn return_connection(&self, host: &str, stream: tokio_rustls::client::TlsStream<TcpStream>) {
        let key = format!("{}:443", host);

        // 先检查是否已存在（不用 entry API，避免持锁时调用 evict）
        if self.connection_pool.contains_key(&key) {
            trace!("DoH 连接池已有 {}, 丢弃当前连接", key);
            return;
        }

        // 池满时先淘汰（此时没有持有任何锁）
        let mut evict_attempts = 0;
        const MAX_EVICT_ATTEMPTS: usize = MAX_POOL_SIZE + 1;

        while self.connection_pool.len() >= MAX_POOL_SIZE && evict_attempts < MAX_EVICT_ATTEMPTS {
            self.evict_oldest_connection();
            evict_attempts += 1;
            if self.connection_pool.is_empty() {
                break;
            }
        }

        // 再插入（可能失败，如果期间有其他线程插入了）
        if self.connection_pool.len() < MAX_POOL_SIZE {
            use dashmap::mapref::entry::Entry;
            match self.connection_pool.entry(key.clone()) {
                Entry::Occupied(_) => {
                    trace!("DoH 连接池已有 {}, 丢弃当前连接", key);
                }
                Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(PoolEntry {
                        stream,
                        last_used: Instant::now(),
                    });
                    if let Ok(mut lru) = self.lru_index.write() {
                        lru.put(key, ());
                    }
                }
            }
        } else {
            debug!("DoH 连接池已满，丢弃新连接");
        }
    }

    fn build_tls_config() -> rustls::ClientConfig {
        let root_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect(),
        };

        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        config.resumption = rustls::client::Resumption::default();
        config
    }

    // 预解析 DoH 服务器地址
    pub fn preresolve_doh_hosts(&self) {
        use std::net::ToSocketAddrs;

        for endpoint in &self.endpoints {
            if let Some(host) = extract_host(&endpoint.url) {
                if let Some(ref preset_ip_str) = endpoint.preset_ip {
                    if let Ok(ip) = preset_ip_str.parse::<IpAddr>() {
                        debug!("DoH 服务器 {} 使用预设 IP: {}", host, ip);
                        self.preresolved_ips.insert(host.to_string(), vec![ip]);
                        continue;
                    }
                }

                let ips: Vec<IpAddr> = format!("{}:443", host)
                    .to_socket_addrs()
                    .map(|addrs| addrs.map(|a| a.ip()).collect())
                    .unwrap_or_default();

                if !ips.is_empty() {
                    debug!("预解析 DoH 服务器 {} -> {:?}", host, ips);
                    self.preresolved_ips.insert(host.to_string(), ips);
                } else {
                    warn!("预解析 DoH 服务器 {} 失败，将使用系统 DNS", host);
                }
            }
        }
    }

    // 查域名 IP
    pub async fn resolve(&self, domain: &str) -> anyhow::Result<DohResolveResult> {
        let groups = self.select_endpoints(domain);
        self.resolve_with_groups(groups, domain).await
    }

    // 仅用国际 DoH 解析（国内 IP 全挂时回退）
    pub async fn resolve_international(&self, domain: &str) -> anyhow::Result<DohResolveResult> {
        let trusted: Vec<&DohEndpoint> = self.endpoints.iter()
            .filter(|e| e.trusted)
            .collect();
        if trusted.is_empty() {
            anyhow::bail!("没有配置国际 DoH 端点");
        }
        let mut sorted = trusted;
        sorted.sort_by_key(|e| e.priority);
        let groups = vec![sorted];
        self.resolve_with_groups(groups, domain).await
    }

    async fn resolve_with_groups(
        &self,
        groups: Vec<Vec<&DohEndpoint>>,
        domain: &str,
    ) -> anyhow::Result<DohResolveResult> {

        let mut all_ips: Vec<IpAddr> = Vec::new();
        let mut doh_servers: Vec<String> = Vec::new();

        for (stage, endpoints) in groups.iter().enumerate() {
            if endpoints.is_empty() {
                continue;
            }
            let (ips, servers) = self.query_group(endpoints, domain).await;
            doh_servers.extend(servers);

            let mut seen: HashSet<_> = all_ips.iter().cloned().collect();
            for ip in ips {
                if seen.insert(ip) {
                    all_ips.push(ip);
                }
            }
            if !all_ips.is_empty() {
                break;
            }
            debug!("第 {} 级 DoH 未返回结果 for {}, 尝试下一级", stage + 1, domain);
        }

        if all_ips.is_empty() {
            anyhow::bail!("No IPs returned from any DoH endpoint for {}", domain);
        }

        Ok(DohResolveResult { ips: all_ips, doh_servers })
    }

    // 查一组端点，返回 (IP 列表, 服务器名列表)
    // 相同 host 的端点共享连接，省重复 TLS 握手
    async fn query_group(
        &self,
        endpoints: &[&DohEndpoint],
        domain: &str,
    ) -> (Vec<IpAddr>, Vec<String>) {
        use futures::stream::{FuturesUnordered, StreamExt};

        // 按 host 分组
        let mut host_to_endpoints: HashMap<&str, Vec<&DohEndpoint>> = HashMap::new();
        for ep in endpoints {
            if let Some(host) = extract_host(&ep.url) {
                host_to_endpoints.entry(host).or_default().push(*ep);
            }
        }

        // 每个 host 并发查，同一 host 内多个端点共享连接
        let mut host_queries: FuturesUnordered<_> = host_to_endpoints
            .into_iter()
            .map(|(host, eps)| self.query_host_grouped(host, eps, domain))
            .collect();

        let mut seen = HashSet::new();
        let mut ips = Vec::new();
        let mut servers = Vec::new();

        // 收集所有 host 的结果
        while let Some(result) = host_queries.next().await {
            if let Ok((host_ips, host_servers)) = result {
                servers.extend(host_servers);
                for ip in host_ips {
                    if seen.insert(ip) {
                        ips.push(ip);
                    }
                }
            }
        }

        (ips, servers)
    }

    // 对同一 host 的多个端点查询（共享连接）
    async fn query_host_grouped(
        &self,
        host: &str,
        endpoints: Vec<&DohEndpoint>,
        domain: &str,
    ) -> anyhow::Result<(Vec<IpAddr>, Vec<String>)> {
        debug!("DoH 查询: {} (via {}, {} 个端点共享连接)", domain, host, endpoints.len());

        // 获取或创建连接
        let mut tls_stream = match self.acquire_connection(host) {
            Some(stream) => stream,
            None => self.create_new_connection(host).await?,
        };

        let mut ips = Vec::new();
        let mut servers = Vec::new();
        let mut seen = HashSet::new();
        let mut connection_ok = true;

        // 在同一连接上顺序查所有端点
        for endpoint in &endpoints {
            let result = self.execute_queries(&mut tls_stream, host, domain).await;

            match result {
                Ok(endpoint_ips) => {
                    if !endpoint_ips.is_empty() {
                        servers.push(endpoint.name.clone());
                        for ip in endpoint_ips {
                            if seen.insert(ip) {
                                ips.push(ip);
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("DoH 查询失败 ({} via {}): {}", endpoint.name, host, e);
                    connection_ok = false;
                    break;  // 连接出问题，停掉后续查询
                }
            }
        }

        // 归还连接（只在连接正常时）
        if connection_ok {
            self.return_connection(host, tls_stream);
        }
        // 连接失败时不归还，让 Rust drop 掉

        Ok((ips, servers))
    }

    // 返回分级端点组：
    // 第一组：核心国内 DoH（2-3 个，优先并发查询）
    // 第二组：备用国内 DoH（核心全挂时才查询）
    // 第三组：国际 DoH（国内全挂时兜底）
    fn select_endpoints(&self, domain: &str) -> Vec<Vec<&DohEndpoint>> {
        let provider_name = self.match_dns_mapping(domain);

        match provider_name {
            Some(name) => {
                let matched: Vec<&DohEndpoint> = self.endpoints
                    .iter()
                    .filter(|e| e.name == name)
                    .collect();

                if matched.is_empty() {
                    warn!("dns_mapping 指定了提供商 '{}' 但未找到对应端点，使用默认分级策略", name);
                    // fall through to default split below
                } else {
                    debug!("域名 {} 使用指定 DoH 提供商: {}", domain, name);
                    return vec![matched];
                }
            }
            None => {}
        }

        // 分离国内/国际端点
        let mut domestic: Vec<&DohEndpoint> = self.endpoints.iter()
            .filter(|e| !e.trusted)
            .collect();
        let mut international: Vec<&DohEndpoint> = self.endpoints.iter()
            .filter(|e| e.trusted)
            .collect();

        domestic.sort_by_key(|e| e.priority);
        international.sort_by_key(|e| e.priority);

        // 国内 DoH 分级：核心 + 备用
        // 核心取前 3 个（最高优先级），备用取剩余
        let (core, backup): (Vec<&DohEndpoint>, Vec<&DohEndpoint>) = if domestic.len() <= 3 {
            (domestic, Vec::new())
        } else {
            let (c, b) = domestic.split_at(3);
            (c.to_vec(), b.to_vec())
        };

        // 返回三级：核心 -> 备用 -> 国际
        let mut groups = Vec::with_capacity(3);
        if !core.is_empty() {
            groups.push(core);
        }
        if !backup.is_empty() {
            groups.push(backup);
        }
        if !international.is_empty() {
            groups.push(international);
        }

        groups
    }

    fn match_dns_mapping(&self, domain: &str) -> Option<String> {
        let domain_lower = domain.to_lowercase();

        if let Some(provider) = self.dns_mapping.get(&domain_lower) {
            return Some(provider.clone());
        }

        for (pattern, provider) in &self.dns_mapping {
            if pattern.starts_with("*.") {
                let suffix = &pattern[1..];
                if domain_lower.ends_with(suffix) {
                    return Some(provider.clone());
                }
            }
        }

        None
    }

    // 新建 TCP+TLS 连接到 DoH 服务器
    async fn create_new_connection(
        &self,
        host: &str,
    ) -> anyhow::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let ip = self.get_doh_server_ip(host).await?;
        let addr = std::net::SocketAddr::new(ip, 443);

        let tcp_stream = tokio::time::timeout(
            DOH_TIMEOUT,
            TcpStream::connect(addr),
        ).await
            .map_err(|_| anyhow::anyhow!("DoH TCP 连接超时 ({})", host))??;
        tcp_stream.set_nodelay(true)?;

        let tls_stream = tokio::time::timeout(
            DOH_TIMEOUT,
            self.establish_tls(tcp_stream, host),
        ).await
            .map_err(|_| anyhow::anyhow!("DoH TLS 握手超时 ({})", host))??;

        Ok(tls_stream)
    }

    // 在已有 TLS 流上执行 A 和 AAAA 查询
    async fn execute_queries(
        &self,
        tls_stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
        host: &str,
        domain: &str,
    ) -> anyhow::Result<Vec<IpAddr>> {
        let query_a = build_dns_query(domain, 0x0001);
        let query_aaaa = build_dns_query(domain, 0x001C);

        let mut ips = Vec::new();

        // A 记录查询 (keep-alive)
        let body = self.send_doh_request(tls_stream, host, &query_a).await?;
        if let Ok(mut a_ips) = parse_dns_response(&body) {
            ips.append(&mut a_ips);
        }

        // AAAA 记录查询 (keep-alive)
        let body = self.send_doh_request(tls_stream, host, &query_aaaa).await?;
        if let Ok(mut aaaa_ips) = parse_dns_response(&body) {
            ips.append(&mut aaaa_ips);
        }

        Ok(ips)
    }

    // 拿 DoH 服务器的 IP
    // 没预解析 IP 时直接报错，避免在离线环境阻塞
    // 要动态解析，初始化时调 preresolve_doh_hosts()
    async fn get_doh_server_ip(&self, host: &str) -> anyhow::Result<IpAddr> {
        if let Some(ips) = self.preresolved_ips.get(host) {
            if let Some(ip) = ips.first() {
                debug!("使用预设/预解析 IP: {} -> {}", host, ip);
                return Ok(*ip);
            }
        }

        // 没预解析 IP，直接报错
        debug!("DoH 服务器 {} 没有预解析 IP，请确保调用了 preresolve_doh_hosts() 或配置了 preset_ip", host);
        anyhow::bail!(
            "DoH 服务器 {} 没有预解析 IP。请确保：\n\
             1. 调用了 preresolve_doh_hosts() 方法，或\n\
             2. 在配置中为该端点设置了 preset_ip",
            host
        )
    }

    async fn establish_tls(
        &self,
        tcp_stream: TcpStream,
        host: &str,
    ) -> anyhow::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let connector = tokio_rustls::TlsConnector::from(self.tls_config.clone());

        let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| anyhow::anyhow!("无效的 SNI 域名 '{}': {}", host, e))?;

        let tls_stream = connector.connect(server_name, tcp_stream).await?;

        debug!("TLS 握手完成 (SNI={}, 正常证书验证)", host);
        Ok(tls_stream)
    }

    async fn send_doh_request(
        &self,
        tls_stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
        host: &str,
        query: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        let query_b64 = base64_encode(query);
        let path = format!("/dns-query?dns={}", query_b64);

        // Always use keep-alive so connections can be returned to the pool
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: application/dns-message\r\nConnection: keep-alive\r\n\r\n",
            path, host
        );

        tls_stream.write_all(request.as_bytes()).await?;
        tls_stream.flush().await?;

        // Read response using Content-Length for precise body extraction
        let mut response = Vec::with_capacity(4096);
        let mut buf = [0u8; 4096];

        // Read until we find the header end marker
        loop {
            if find_header_end(&response).is_some() {
                break;
            }
            let n = tls_stream.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            response.extend_from_slice(&buf[..n]);
        }

        // Parse Content-Length
        let content_length = parse_content_length(&response).unwrap_or(0);
        let header_end = find_header_end(&response).unwrap_or(0);
        let body_start = header_end + 4; // skip \r\n\r\n
        let body_received = response.len().saturating_sub(body_start);

        // Continue reading until body is complete
        let mut total_body = body_received;
        while total_body < content_length {
            let n = tls_stream.read(&mut buf).await?;
            if n == 0 { break; }
            response.extend_from_slice(&buf[..n]);
            total_body += n;
        }

        let body = extract_http_body(&response)?;
        Ok(body.to_vec())
    }
}

fn extract_host(url: &str) -> Option<&str> {
    let without_scheme = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let host = without_scheme.split('/').next()?;
    Some(host.split(':').next()?)
}

// 找 HTTP 头结束位置（\r\n\r\n 的起始索引）
fn find_header_end(data: &[u8]) -> Option<usize> {
    let separator = b"\r\n\r\n";
    data.windows(separator.len())
        .position(|w| w == separator)
}

// 从 HTTP 响应头解析 Content-Length
fn parse_content_length(response: &[u8]) -> Option<usize> {
    let header_end = find_header_end(response)?;
    let header_str = std::str::from_utf8(&response[..header_end]).ok()?;

    for line in header_str.lines() {
        if let Some(value) = line.strip_prefix("Content-Length:") {
            return value.trim().parse().ok();
        }
        if let Some(value) = line.strip_prefix("content-length:") {
            return value.trim().parse().ok();
        }
    }
    None
}

fn extract_http_body(response: &[u8]) -> anyhow::Result<&[u8]> {
    let separator = b"\r\n\r\n";
    let pos = response
        .windows(separator.len())
        .position(|w| w == separator)
        .ok_or_else(|| anyhow::anyhow!("无效的 HTTP 响应：找不到 header/body 分隔"))?;

    Ok(&response[pos + separator.len()..])
}

fn base64_encode(data: &[u8]) -> String {
    let mut encoded = String::new();
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut i = 0;
    while i + 2 < data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        encoded.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        encoded.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }

    let remaining = data.len() - i;
    if remaining == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        encoded.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
    } else if remaining == 1 {
        let n = (data[i] as u32) << 16;
        encoded.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
    }

    encoded
}

fn build_dns_query(domain: &str, qtype: u16) -> Vec<u8> {
    use rand::Rng;

    let mut buf = Vec::with_capacity(64);

    let mut rng = rand::thread_rng();
    let id: u16 = rng.gen();

    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes());
    buf.extend_from_slice(&0x0001u16.to_be_bytes());
    buf.extend_from_slice(&0x0000u16.to_be_bytes());
    buf.extend_from_slice(&0x0000u16.to_be_bytes());
    buf.extend_from_slice(&0x0000u16.to_be_bytes());

    encode_domain_name(domain, &mut buf);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&0x0001u16.to_be_bytes());

    buf
}

fn encode_domain_name(domain: &str, buf: &mut Vec<u8>) {
    for label in domain.split('.') {
        let label_bytes = label.as_bytes();
        buf.push(label_bytes.len() as u8);
        buf.extend_from_slice(label_bytes);
    }
    buf.push(0);
}

fn parse_dns_response(data: &[u8]) -> anyhow::Result<Vec<IpAddr>> {
    let mut ips = Vec::new();

    if data.len() < 12 {
        anyhow::bail!("DNS response too short");
    }

    let _id = u16::from_be_bytes([data[0], data[1]]);
    let flags = u16::from_be_bytes([data[2], data[3]]);
    let _qdcount = u16::from_be_bytes([data[4], data[5]]);
    let ancount = u16::from_be_bytes([data[6], data[7]]);

    let rcode = flags & 0x000F;
    if rcode != 0 {
        anyhow::bail!("DNS error: rcode {}", rcode);
    }

    let mut offset = 12;

    while offset < data.len() && data[offset] != 0 {
        offset += data[offset] as usize + 1;
    }
    offset += 5;

    for _ in 0..ancount {
        if offset + 12 > data.len() {
            break;
        }

        if data[offset] & 0xC0 == 0xC0 {
            offset += 2;
        } else {
            while offset < data.len() && data[offset] != 0 {
                offset += data[offset] as usize + 1;
            }
            offset += 1;
        }

        let rtype = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let _rclass = u16::from_be_bytes([data[offset + 2], data[offset + 3]]);
        let _ttl = u32::from_be_bytes([data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]]);
        let rdlength = u16::from_be_bytes([data[offset + 8], data[offset + 9]]) as usize;

        offset += 10;

        match rtype {
            1 => {
                if offset + 4 <= data.len() {
                    let ip = std::net::Ipv4Addr::new(
                        data[offset],
                        data[offset + 1],
                        data[offset + 2],
                        data[offset + 3],
                    );
                    ips.push(IpAddr::V4(ip));
                }
            }
            28 => {
                if offset + 16 <= data.len() {
                    let ip = std::net::Ipv6Addr::new(
                        u16::from_be_bytes([data[offset], data[offset + 1]]),
                        u16::from_be_bytes([data[offset + 2], data[offset + 3]]),
                        u16::from_be_bytes([data[offset + 4], data[offset + 5]]),
                        u16::from_be_bytes([data[offset + 6], data[offset + 7]]),
                        u16::from_be_bytes([data[offset + 8], data[offset + 9]]),
                        u16::from_be_bytes([data[offset + 10], data[offset + 11]]),
                        u16::from_be_bytes([data[offset + 12], data[offset + 13]]),
                        u16::from_be_bytes([data[offset + 14], data[offset + 15]]),
                    );
                    ips.push(IpAddr::V6(ip));
                }
            }
            _ => {}
        }

        offset += rdlength;
    }

    Ok(ips)
}

