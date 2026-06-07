use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, trace, warn};

use crate::cache::ResourceCache;
use crate::core::config::InterceptAction;
use crate::intercepts::matcher::InterceptMatcher;
use crate::intercepts::mirror_health::MirrorHealthTracker;
use crate::mitm::ca::CertificateAuthority;
use crate::mitm::cert_cache::CertCache;
use crate::mitm::tls_origin::TlsOrigin;
use crate::mitm::tls_terminator::TlsTerminator;
use crate::proxy::metrics::Metrics;
use crate::rules::builtin::{DomainAction, DomainRules};
use crate::system::pac::generate_pac;

pub struct HandlerContext {
    pub mitm_enabled: bool,
    pub ca: Arc<CertificateAuthority>,
    pub cert_cache: Arc<CertCache>,
    pub tls_terminator: Arc<TlsTerminator>,
    pub tls_origin: Arc<TlsOrigin>,
    pub domain_rules: Arc<DomainRules>,
    pub intercept_matcher: Arc<InterceptMatcher>,
    pub mirror_health: Arc<MirrorHealthTracker>,
    pub metrics: Arc<Metrics>,
    pub resource_cache: Arc<ResourceCache>,
    pub default_fake_sni: String,
    pub sni_spoof_enabled: bool,
    pub proxy_port: u16,
    pub idle_timeout_secs: u64,
}

impl HandlerContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mitm_enabled: bool,
        ca: Arc<CertificateAuthority>,
        cert_cache: Arc<CertCache>,
        tls_terminator: Arc<TlsTerminator>,
        tls_origin: Arc<TlsOrigin>,
        domain_rules: Arc<DomainRules>,
        intercept_matcher: Arc<InterceptMatcher>,
        mirror_health: Arc<MirrorHealthTracker>,
        metrics: Arc<Metrics>,
        resource_cache: Arc<ResourceCache>,
        default_fake_sni: String,
        sni_spoof_enabled: bool,
        proxy_port: u16,
        idle_timeout_secs: u64,
    ) -> Self {
        Self {
            mitm_enabled,
            ca,
            cert_cache,
            tls_terminator,
            tls_origin,
            domain_rules,
            intercept_matcher,
            mirror_health,
            metrics,
            resource_cache,
            default_fake_sni,
            sni_spoof_enabled,
            proxy_port,
            idle_timeout_secs,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProxyAction {
    HttpForward {
        host: String,
        port: u16,
    },
    HttpsTunnel {
        host: String,
        port: u16,
        use_doh: bool,
    },
    MitmIntercept {
        host: String,
        port: u16,
    },
}

// 1 小时超时，防止僵死连接
const BIDIRECTIONAL_TIMEOUT_SECS: u64 = 3600;

/// 带流量统计的双向拷贝
async fn copy_bidirectional_with_metrics<A, B>(
    a: &mut A,
    b: &mut B,
    metrics: &Arc<Metrics>,
) -> std::io::Result<(u64, u64)>
where
    A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut a_to_b = 0u64;
    let mut b_to_a = 0u64;
    let mut a_buf = [0u8; 32768];
    let mut b_buf = [0u8; 32768];

    loop {
        tokio::select! {
            result = a.read(&mut a_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        a_to_b += n as u64;
                        metrics.add_bytes_sent(n as u64);
                        b.write_all(&a_buf[..n]).await?;
                    }
                    Err(e) => return Err(e),
                }
            }
            result = b.read(&mut b_buf) => {
                match result {
                    Ok(0) => break,
                    Ok(n) => {
                        b_to_a += n as u64;
                        metrics.add_bytes_received(n as u64);
                        a.write_all(&b_buf[..n]).await?;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    Ok((a_to_b, b_to_a))
}

pub async fn handle_connection(stream: TcpStream, ctx: Arc<HandlerContext>) -> anyhow::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.peek(&mut buf).await?;

    if n == 0 {
        return Ok(());
    }

    let first_bytes = &buf[..n];

    if first_bytes.starts_with(b"CONNECT") {
        handle_connect(stream, &ctx).await
    } else if is_http_method(first_bytes) {
        handle_http(stream, &ctx).await
    } else {
        warn!("未知协议，关闭连接");
        Ok(())
    }
}

fn is_http_method(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    match bytes[0] {
        b'G' => bytes.starts_with(b"GET"),
        b'P' => {
            bytes.starts_with(b"POST") || bytes.starts_with(b"PUT") || bytes.starts_with(b"PATCH")
        }
        b'D' => bytes.starts_with(b"DELETE"),
        b'H' => bytes.starts_with(b"HEAD"),
        b'O' => bytes.starts_with(b"OPTIONS"),
        _ => false,
    }
}

pub async fn handle_connect(mut client: TcpStream, ctx: &HandlerContext) -> anyhow::Result<()> {
    let mut request_buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];

    loop {
        let n = client.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        request_buf.extend_from_slice(&tmp[..n]);

        if contains_header_end(&request_buf) {
            break;
        }

        if request_buf.len() > 8192 {
            warn!("CONNECT 请求过长，中止处理");
            return Ok(());
        }
    }

    let request_str = String::from_utf8_lossy(&request_buf);
    let first_line = request_str.lines().next().unwrap_or("");

    let target = parse_connect_target(first_line);
    match target {
        Some((host, port)) => {
            info!("CONNECT {}:{}", host, port);

            let domain_action = ctx.domain_rules.classify(&host);

            if let Some(InterceptAction::Abort(_)) = ctx.intercept_matcher.match_domain(&host, "/")
            {
                info!("拦截: 屏蔽 {}", host);
                client
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                return Ok(());
            }

            let action = match domain_action {
                DomainAction::Proxy if ctx.mitm_enabled => ProxyAction::MitmIntercept {
                    host: host.clone(),
                    port,
                },
                DomainAction::Proxy => ProxyAction::HttpsTunnel {
                    host: host.clone(),
                    port,
                    use_doh: true,
                },
                DomainAction::Direct => ProxyAction::HttpsTunnel {
                    host: host.clone(),
                    port,
                    use_doh: false,
                },
                DomainAction::Block => {
                    info!("已屏蔽对 {}:{} 的 CONNECT 请求", host, port);
                    client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await?;
                    return Ok(());
                }
            };

            match action {
                ProxyAction::HttpsTunnel {
                    host,
                    port,
                    use_doh,
                } => {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;

                    crate::proxy::tunnel::establish_tunnel(
                        client,
                        &host,
                        port,
                        &ctx.tls_origin,
                        use_doh,
                        &ctx.metrics,
                    )
                    .await?;
                }
                ProxyAction::MitmIntercept { host, port } => {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;

                    if let Err(e) = perform_mitm(client, &host, port, ctx).await {
                        warn!("MITM 失败 {}:{}: {}", host, port, e);
                    }
                }
                _ => unreachable!(),
            }
        }
        None => {
            warn!("无法解析 CONNECT 请求: {}", first_line);
            client
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
        }
    }

    Ok(())
}

async fn perform_mitm(
    client: TcpStream,
    host: &str,
    port: u16,
    ctx: &HandlerContext,
) -> anyhow::Result<()> {
    use tokio::io::copy_bidirectional;

    info!("MITM: 开始拦截 {}:{}", host, port);

    let mut client_tls = match ctx.tls_terminator.accept(client, host).await {
        Ok(stream) => stream,
        Err(e) => {
            warn!("MITM: 客户端 TLS 握手失败 {}: {}", host, e);
            return Err(e);
        }
    };

    info!("MITM: 客户端 TLS 已建立 {}", host);

    let mut peek_buf = [0u8; 8192];
    let n = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client_tls.read(&mut peek_buf),
    )
    .await
    {
        Ok(Ok(n)) if n > 0 => n,
        Ok(Ok(_)) => {
            info!("MITM: HTTP 请求读取到空数据 {}", host);
            return Ok(());
        }
        Err(_) => {
            info!("MITM: HTTP 请求读取超时 {} (30s)", host);
            return Ok(());
        }
        Ok(Err(e)) => {
            info!("MITM: HTTP 请求读取错误 {}: {}", host, e);
            return Err(e.into());
        }
    };

    let pending_request = peek_buf[..n].to_vec();
    let request_str = String::from_utf8_lossy(&peek_buf[..n]);

    let path = request_str
        .lines()
        .next()
        .and_then(parse_request_path)
        .unwrap_or_else(|| "/".to_string());

    info!("MITM: HTTP 请求 {} {}", host, path);

    let upstream_decision = resolve_upstream(host, &path, ctx, &pending_request);

    match upstream_decision {
        UpstreamDecision::Redirect { response } => {
            client_tls.write_all(&response).await?;
            info!("MITM: HTTP 重定向已发送 {}", host);
            return Ok(());
        }

        UpstreamDecision::ProxyMirror {
            mirror,
            rewritten_request,
        } => {
            info!("MITM: 镜像代理: {} -> {}", host, mirror);

            let connect_start = Instant::now();
            let mut server_tls = match ctx.tls_origin.connect_with_fallback(&mirror).await {
                Ok(stream) => stream,
                Err(e) => {
                    ctx.mirror_health.record_failure(&mirror);
                    warn!("镜像站 {} 连接失败，回退到原域名 {}: {}", mirror, host, e);
                    match ctx.tls_origin.connect_with_fallback(host).await {
                        Ok(stream) => stream,
                        Err(e2) => {
                            warn!("MITM: 原域名 {} 也连接失败: {}", host, e2);
                            return Err(e2);
                        }
                    }
                }
            };

            let response_time = connect_start.elapsed().as_secs_f64() * 1000.0;
            ctx.mirror_health.record_success(&mirror, response_time);

            server_tls.write_all(&rewritten_request).await?;
            info!(
                "MITM: 已转发改写后的请求 {} 字节到 {}",
                rewritten_request.len(),
                mirror
            );

            let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
            match tokio::time::timeout(
                timeout,
                copy_bidirectional(&mut client_tls, &mut server_tls),
            )
            .await
            {
                Ok(Ok((c2s, s2c))) => {
                    ctx.metrics.add_bytes_sent(c2s);
                    ctx.metrics.add_bytes_received(s2c);
                    info!(
                        "MITM: 镜像隧道关闭 {}, 客户端->服务器 {} 字节, 服务器->客户端 {} 字节",
                        host, c2s, s2c
                    );
                }
                Ok(Err(e)) => {
                    if e.to_string().contains("close_notify") {
                        trace!("MITM: 镜像连接正常关闭 {}: {}", host, e);
                    } else {
                        info!("MITM: 镜像双向复制错误 {}: {}", host, e);
                    }
                }
                Err(_) => {
                    warn!(
                        "MITM: 镜像传输超时 {} ({}s)",
                        host, BIDIRECTIONAL_TIMEOUT_SECS
                    );
                }
            }
        }

        UpstreamDecision::SniSpoof {
            real_host,
            fake_sni,
        } => {
            info!("MITM: SNI 伪装 {} (SNI={})", real_host, fake_sni);

            // 构建完整 URL 用于缓存查找
            let url = format!("https://{}{}", real_host, path);
            let should_cache = ResourceCache::is_cacheable_url(&url);
            debug!(
                "MITM: SNI 伪装缓存检查 url={}, should_cache={}",
                url, should_cache
            );

            // 尝试从缓存获取（仅对静态资源）
            if should_cache {
                if let Some(cached) = ctx.resource_cache.get(&url).await {
                    info!("MITM: SNI 伪装缓存命中 {}", url);
                    let response = build_response_from_cache(&cached);
                    client_tls.write_all(&response).await?;
                    ctx.metrics.add_bytes_received(cached.data.len() as u64);
                    return Ok(());
                }
            }

            // 检查是否是 GitHub releases expanded_assets 页面（需要注入脚本）
            // 只匹配 /releases/expanded_assets/ 页面，其他页面正常透传
            let request_str = String::from_utf8_lossy(&pending_request);
            let is_assets_page =
                real_host == "github.com" && request_str.contains("/releases/expanded_assets/");

            let mut server_tls = match ctx
                .tls_origin
                .connect_with_sni_spoof(&real_host, &fake_sni)
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    warn!("MITM: SNI 伪装连接失败 {}: {}", real_host, e);
                    match ctx.tls_origin.connect_with_fallback(&real_host).await {
                        Ok(stream) => stream,
                        Err(e2) => {
                            warn!("MITM: 回退连接也失败 {}: {}", real_host, e2);
                            return Err(e2);
                        }
                    }
                }
            };

            info!("MITM: SNI 伪装连接已建立 {} (SNI={})", real_host, fake_sni);

            if !pending_request.is_empty() {
                server_tls.write_all(&pending_request).await?;
                info!(
                    "MITM: 已转发客户端请求 {} 字节到 {}",
                    pending_request.len(),
                    real_host
                );
            }

            // 如果是 GitHub releases/assets 页面，需要拦截响应并注入脚本
            // 注意：仅对 HTML 页面注入，下载文件直接透传
            if is_assets_page {
                info!("MITM: 检测到 GitHub releases/assets 页面，检查是否需要注入脚本");

                // 先读取响应头，判断 Content-Type
                let mut raw_buf = Vec::with_capacity(65536);
                let mut tmp = [0u8; 8192];

                // 读取直到找到响应头结束 (\r\n\r\n)
                let mut header_end_pos: Option<usize> = None;
                loop {
                    match server_tls.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => {
                            raw_buf.extend_from_slice(&tmp[..n]);
                            // 检查是否已读取完整响应头
                            if let Some(pos) = find_header_end(&raw_buf) {
                                header_end_pos = Some(pos);
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                    if raw_buf.len() > 65536 {
                        warn!("MITM: 响应头过大，放弃注入");
                        break;
                    }
                }

                // 分离响应头和已读取的响应体
                let (header_buf, body_already_read) = if let Some(pos) = header_end_pos {
                    let header = raw_buf[..pos + 4].to_vec();
                    let body = raw_buf[pos + 4..].to_vec();
                    (header, body)
                } else {
                    (raw_buf, Vec::new())
                };

                // 检查 Content-Type 是否是 HTML
                let response_str = String::from_utf8_lossy(&header_buf);
                let is_html = response_str.to_lowercase().contains("content-type:")
                    && response_str.to_lowercase().contains("text/html");

                // 检查是否是 gzip 压缩（不能直接注入）
                let is_gzip = response_str.to_lowercase().contains("content-encoding:")
                    && (response_str.to_lowercase().contains("gzip")
                        || response_str.to_lowercase().contains("deflate"));

                if is_html && !is_gzip {
                    info!("MITM: 检测到 HTML 响应，读取完整内容并注入脚本");

                    // 解析 Content-Length 来确定需要读取多少字节
                    let content_length = parse_content_length(&header_buf);
                    let is_chunked = response_str.to_lowercase().contains("transfer-encoding:")
                        && response_str.to_lowercase().contains("chunked");

                    debug!(
                        "MITM: Content-Length={:?}, chunked={}, header_buf={}",
                        content_length,
                        is_chunked,
                        header_buf.len()
                    );

                    // 读取剩余响应体
                    let mut body_buf = body_already_read;

                    if let Some(total_len) = content_length {
                        // 有 Content-Length，按长度读取
                        let remaining = if total_len > body_buf.len() as u64 {
                            total_len - body_buf.len() as u64
                        } else {
                            0
                        };

                        let mut read_total = 0u64;
                        while read_total < remaining {
                            let to_read =
                                std::cmp::min(tmp.len(), (remaining - read_total) as usize);
                            match server_tls.read(&mut tmp[..to_read]).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    body_buf.extend_from_slice(&tmp[..n]);
                                    read_total += n as u64;
                                }
                                Err(_) => break,
                            }
                        }
                    } else if is_chunked {
                        // chunked 编码，读取到 0\r\n\r\n 结束标记
                        let read_timeout = Duration::from_secs(5);
                        loop {
                            match tokio::time::timeout(read_timeout, server_tls.read(&mut tmp))
                                .await
                            {
                                Ok(Ok(0)) => break,
                                Ok(Ok(n)) => {
                                    body_buf.extend_from_slice(&tmp[..n]);
                                    // 检查是否读到 chunked 结束标记 0\r\n\r\n
                                    if body_buf.ends_with(b"0\r\n\r\n")
                                        || body_buf.windows(5).any(|w| w == b"0\r\n\r\n")
                                    {
                                        debug!("MITM: 检测到 chunked 结束标记");
                                        break;
                                    }
                                }
                                Ok(Err(_)) => break,
                                Err(_) => {
                                    debug!("MITM: chunked 读取超时");
                                    break;
                                }
                            }
                            if body_buf.len() > 2 * 1024 * 1024 {
                                break;
                            }
                        }
                    } else {
                        // 没有 Content-Length 也不是 chunked，读取到连接关闭
                        // 设置短超时避免无限等待
                        let read_timeout = Duration::from_secs(3);
                        loop {
                            match tokio::time::timeout(read_timeout, server_tls.read(&mut tmp))
                                .await
                            {
                                Ok(Ok(0)) => break,
                                Ok(Ok(n)) => body_buf.extend_from_slice(&tmp[..n]),
                                Ok(Err(_)) => break,
                                Err(_) => {
                                    debug!("MITM: 读取响应体超时，假设已完成");
                                    break;
                                }
                            }
                            if body_buf.len() > 2 * 1024 * 1024 {
                                break;
                            }
                        }
                    }

                    // 合并响应头和响应体
                    let mut full_response = header_buf.clone();
                    full_response.extend_from_slice(&body_buf);

                    // 注入脚本
                    let full_str = String::from_utf8_lossy(&full_response);
                    let modified_html = inject_mirror_script(&full_str);

                    // 发送修改后的响应
                    client_tls.write_all(modified_html.as_bytes()).await?;
                    info!("MITM: 已注入镜像脚本并转发 {} 字节", modified_html.len());
                } else {
                    // 不是 HTML 或是 gzip 压缩，直接透传
                    if is_gzip {
                        info!("MITM: 响应已 gzip 压缩，跳过注入直接透传");
                    } else {
                        info!("MITM: 非 HTML 响应，直接透传");
                    }

                    // 先发送响应头和已读取的响应体
                    client_tls.write_all(&header_buf).await?;
                    if !body_already_read.is_empty() {
                        client_tls.write_all(&body_already_read).await?;
                        debug!(
                            "MITM: 已转发已读取的响应体 {} 字节",
                            body_already_read.len()
                        );
                    }

                    // 继续双向透传剩余数据
                    match copy_bidirectional_with_metrics(
                        &mut client_tls,
                        &mut server_tls,
                        &ctx.metrics,
                    )
                    .await
                    {
                        Ok((c2s, s2c)) => {
                            info!("MITM: SNI 伪装隧道关闭 {}, 客户端->服务器 {} 字节, 服务器->客户端 {} 字节", real_host, c2s, s2c);
                        }
                        Err(e) => {
                            if !e.to_string().contains("close_notify") {
                                info!("MITM: SNI 伪装双向复制错误 {}: {}", real_host, e);
                            }
                        }
                    }
                }
            } else {
                // 非 release 页面，正常转发（支持缓存）
                if should_cache {
                    // 读取响应并缓存
                    let mut response_buf = Vec::with_capacity(65536);
                    let mut tmp = [0u8; 8192];

                    // 读取响应头（直到 \r\n\r\n），带超时
                    let header_read_timeout = Duration::from_secs(30);
                    let header_end = match tokio::time::timeout(header_read_timeout, async {
                        loop {
                            let n = match server_tls.read(&mut tmp).await {
                                Ok(0) => return Ok(None),
                                Ok(n) => n,
                                Err(e) => return Err(e),
                            };

                            response_buf.extend_from_slice(&tmp[..n]);

                            if let Some(pos) = find_header_end(&response_buf) {
                                return Ok(Some(pos));
                            }

                            if response_buf.len() > 65536 {
                                return Ok(None);
                            }
                        }
                    })
                    .await
                    {
                        Ok(Ok(Some(pos))) => Some(pos),
                        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                            warn!("MITM: SNI 伪装响应头读取失败/超时，放弃缓存直接透传");
                            if !response_buf.is_empty() {
                                client_tls.write_all(&response_buf).await?;
                                ctx.metrics.add_bytes_received(response_buf.len() as u64);
                            }
                            let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                            let _ = tokio::time::timeout(
                                timeout,
                                copy_bidirectional(&mut client_tls, &mut server_tls),
                            )
                            .await;
                            return Ok(());
                        }
                    };

                    // 解析响应头
                    let (content_type, content_length) = parse_response_headers(&response_buf);

                    // 发送已读取的数据给客户端
                    client_tls.write_all(&response_buf).await?;
                    ctx.metrics.add_bytes_received(response_buf.len() as u64);

                    // 判断是否可缓存（必须有 Content-Length 才能缓存，否则无法确定响应体结束）
                    let can_cache = content_length.is_some()
                        && content_type
                            .as_ref()
                            .map(|ct| ResourceCache::is_cacheable(&url, ct, content_length))
                            .unwrap_or(false);
                    debug!("MITM: SNI 伪装缓存判断 url={}, content_type={:?}, content_length={:?}, can_cache={}", url, content_type, content_length, can_cache);

                    if can_cache {
                        // 有 Content-Length，精确读取指定字节数
                        let total_body = content_length.unwrap() as usize;
                        let mut body = if let Some(pos) = header_end {
                            response_buf[pos + 4..].to_vec()
                        } else {
                            Vec::new()
                        };
                        body.reserve(total_body.saturating_sub(body.len()));

                        // 精确读取剩余字节
                        while body.len() < total_body {
                            let remaining = total_body - body.len();
                            let to_read = std::cmp::min(tmp.len(), remaining);
                            let n = match server_tls.read(&mut tmp[..to_read]).await {
                                Ok(0) => break,
                                Ok(n) => n,
                                Err(_) => break,
                            };
                            body.extend_from_slice(&tmp[..n]);
                            client_tls.write_all(&tmp[..n]).await?;
                            ctx.metrics.add_bytes_received(n as u64);
                        }

                        // 存入缓存
                        if body.len() == total_body {
                            if let Some(ct) = content_type {
                                let cached = crate::cache::CachedResource {
                                    content_type: ct,
                                    data: body,
                                    etag: None,
                                    last_modified: None,
                                };
                                if let Err(e) = ctx.resource_cache.put(&url, &cached).await {
                                    warn!("MITM: SNI 伪装缓存存储失败: {}", e);
                                } else {
                                    info!(
                                        "MITM: SNI 伪装已缓存 {} ({} bytes)",
                                        url,
                                        cached.data.len()
                                    );
                                }
                            }
                        } else {
                            debug!(
                                "MITM: SNI 伪装响应体不完整 (期望 {} 字节，实际 {} 字节)，放弃缓存",
                                total_body,
                                body.len()
                            );
                        }

                        // 继续透传连接剩余数据（keep-alive 可能有后续请求）
                        let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                        let _ = tokio::time::timeout(
                            timeout,
                            copy_bidirectional(&mut client_tls, &mut server_tls),
                        )
                        .await;
                    } else {
                        // 不可缓存或无 Content-Length，继续透传
                        let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                        let _ = tokio::time::timeout(
                            timeout,
                            copy_bidirectional(&mut client_tls, &mut server_tls),
                        )
                        .await;
                    }
                } else {
                    // 不需要缓存，直接透传
                    match copy_bidirectional_with_metrics(
                        &mut client_tls,
                        &mut server_tls,
                        &ctx.metrics,
                    )
                    .await
                    {
                        Ok((c2s, s2c)) => {
                            info!("MITM: SNI 伪装隧道关闭 {}, 客户端->服务器 {} 字节, 服务器->客户端 {} 字节", real_host, c2s, s2c);
                        }
                        Err(e) => {
                            if e.to_string().contains("close_notify") {
                                trace!("MITM: SNI 伪装连接正常关闭 {}: {}", real_host, e);
                            } else {
                                info!("MITM: SNI 伪装双向复制错误 {}: {}", real_host, e);
                            }
                        }
                    }
                }
            }
        }

        UpstreamDecision::Direct => {
            // 构建完整 URL 用于缓存查找
            let url = format!("https://{}{}", host, path);

            // 尝试从缓存获取（仅对 GitHub 图片）
            if ResourceCache::is_cacheable_url(&url) {
                if let Some(cached) = ctx.resource_cache.get(&url).await {
                    info!("MITM: 缓存命中 {}", url);

                    // 构建响应头
                    let response = build_response_from_cache(&cached);
                    client_tls.write_all(&response).await?;
                    ctx.metrics.add_bytes_received(cached.data.len() as u64);
                    return Ok(());
                }
            }

            let mut server_tls = match ctx.tls_origin.connect_with_fallback(host).await {
                Ok(stream) => stream,
                Err(e) => {
                    warn!("MITM: 所有 IP 均连接失败 {}: {}", host, e);
                    return Err(e);
                }
            };

            info!("MITM: 服务器 TLS 已建立 {}", host);

            if !pending_request.is_empty() {
                server_tls.write_all(&pending_request).await?;
                info!(
                    "MITM: 已转发客户端请求 {} 字节到 {}",
                    pending_request.len(),
                    host
                );
            }

            // 尝试缓存响应
            let should_cache = ResourceCache::is_cacheable_url(&url);

            if should_cache {
                // 读取响应头
                let mut response_buf = Vec::with_capacity(65536);
                let mut tmp = [0u8; 8192];
                let mut header_end = None;

                // 读取响应头（直到 \r\n\r\n）
                loop {
                    let n = match server_tls.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) => {
                            warn!("MITM: 读取响应头失败: {}", e);
                            return Err(e.into());
                        }
                    };

                    response_buf.extend_from_slice(&tmp[..n]);

                    // 查找响应头结束位置
                    if let Some(pos) = find_header_end(&response_buf) {
                        header_end = Some(pos);
                        break;
                    }

                    if response_buf.len() > 65536 {
                        warn!("MITM: 响应头过大，放弃缓存");
                        break;
                    }
                }

                // 解析响应头
                let (content_type, content_length) = parse_response_headers(&response_buf);

                // 发送已读取的数据给客户端
                client_tls.write_all(&response_buf).await?;
                ctx.metrics.add_bytes_received(response_buf.len() as u64);

                // 判断是否可缓存（必须有 Content-Length）
                let can_cache = content_length.is_some()
                    && content_type
                        .as_ref()
                        .map(|ct| ResourceCache::is_cacheable(&url, ct, content_length))
                        .unwrap_or(false);

                if can_cache {
                    // 有 Content-Length，精确读取指定字节数
                    let total_body = content_length.unwrap() as usize;
                    let mut body = if let Some(pos) = header_end {
                        response_buf[pos + 4..].to_vec()
                    } else {
                        Vec::new()
                    };
                    body.reserve(total_body.saturating_sub(body.len()));

                    // 精确读取剩余字节
                    while body.len() < total_body {
                        let remaining = total_body - body.len();
                        let to_read = std::cmp::min(tmp.len(), remaining);
                        let n = match server_tls.read(&mut tmp[..to_read]).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        body.extend_from_slice(&tmp[..n]);
                        client_tls.write_all(&tmp[..n]).await?;
                        ctx.metrics.add_bytes_received(n as u64);
                    }

                    // 存入缓存
                    if body.len() == total_body {
                        if let Some(ct) = content_type {
                            let cached = crate::cache::CachedResource {
                                content_type: ct,
                                data: body,
                                etag: None,
                                last_modified: None,
                            };
                            if let Err(e) = ctx.resource_cache.put(&url, &cached).await {
                                warn!("MITM: 缓存存储失败: {}", e);
                            } else {
                                info!("MITM: 已缓存 {} ({} bytes)", url, cached.data.len());
                            }
                        }
                    }

                    // 继续透传连接剩余数据
                    let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                    let _ = tokio::time::timeout(
                        timeout,
                        copy_bidirectional(&mut client_tls, &mut server_tls),
                    )
                    .await;
                } else {
                    // 不可缓存，继续透传
                    let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                    let _ = tokio::time::timeout(
                        timeout,
                        copy_bidirectional(&mut client_tls, &mut server_tls),
                    )
                    .await;
                }
            } else {
                // 不需要缓存，直接透传
                let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
                match tokio::time::timeout(
                    timeout,
                    copy_bidirectional(&mut client_tls, &mut server_tls),
                )
                .await
                {
                    Ok(Ok((c2s, s2c))) => {
                        ctx.metrics.add_bytes_sent(c2s);
                        ctx.metrics.add_bytes_received(s2c);
                        info!(
                            "MITM: 隧道关闭 {}, 客户端->服务器 {} 字节, 服务器->客户端 {} 字节",
                            host, c2s, s2c
                        );
                    }
                    Ok(Err(e)) => {
                        if e.to_string().contains("close_notify") {
                            trace!("MITM: 连接正常关闭 {}: {}", host, e);
                        } else {
                            info!("MITM: 双向复制错误 {}: {}", host, e);
                        }
                    }
                    Err(_) => {
                        warn!("MITM: 传输超时 {} ({}s)", host, BIDIRECTIONAL_TIMEOUT_SECS);
                    }
                }
            }
        }

        UpstreamDecision::Abort => {
            return Ok(());
        }
    }

    Ok(())
}

enum UpstreamDecision {
    Redirect {
        response: Vec<u8>,
    },
    ProxyMirror {
        mirror: String,
        rewritten_request: Vec<u8>,
    },
    SniSpoof {
        real_host: String,
        fake_sni: String,
    },
    Direct,
    Abort,
}

fn resolve_upstream(
    host: &str,
    path: &str,
    ctx: &HandlerContext,
    pending_request: &[u8],
) -> UpstreamDecision {
    let resolve_real_host = |rule_host: &str, actual_host: &str| -> String {
        if rule_host.contains('*') {
            actual_host.to_string()
        } else {
            rule_host.to_string()
        }
    };

    match ctx.intercept_matcher.match_domain(host, path) {
        Some(InterceptAction::SniSpoof { sni, real_host }) => {
            if ctx.sni_spoof_enabled {
                UpstreamDecision::SniSpoof {
                    real_host: resolve_real_host(real_host, host),
                    fake_sni: sni.clone(),
                }
            } else {
                UpstreamDecision::Direct
            }
        }

        Some(InterceptAction::Proxy { mirror }) => {
            let rewritten = rewrite_request_for_mirror(pending_request, mirror, host);
            UpstreamDecision::ProxyMirror {
                mirror: mirror.clone(),
                rewritten_request: rewritten,
            }
        }

        Some(InterceptAction::Redirect { target }) => {
            let target_url = target.replace("{path}", path);
            let response = build_redirect_response(&target_url);
            UpstreamDecision::Redirect { response }
        }

        Some(InterceptAction::Abort(_)) => UpstreamDecision::Abort,

        None => UpstreamDecision::Direct,
    }
}

async fn handle_http(mut client: TcpStream, ctx: &HandlerContext) -> anyhow::Result<()> {
    let mut request_buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 4096];

    loop {
        let n = client.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        request_buf.extend_from_slice(&tmp[..n]);

        if contains_header_end(&request_buf) {
            break;
        }

        if request_buf.len() > 65536 {
            warn!("HTTP 请求头超过 64KB 限制");
            client
                .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\n\r\n")
                .await?;
            return Ok(());
        }
    }

    let request_str = String::from_utf8_lossy(&request_buf);
    let first_line = request_str.lines().next().unwrap_or("");

    // 处理 PAC 文件请求
    if is_pac_request(first_line) {
        let pac_content = generate_pac(ctx.proxy_port);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            pac_content.len(),
            pac_content
        );
        client.write_all(response.as_bytes()).await?;
        debug!("已返回 PAC 文件");
        return Ok(());
    }

    let target = parse_http_target(first_line);
    match target {
        Some((host, port)) => {
            info!("HTTP 转发到 {}:{}", host, port);

            let action = ctx.domain_rules.classify(&host);

            match action {
                DomainAction::Block => {
                    client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await?;
                }
                _ => match forward_http(client, &request_str, &host, port, ctx).await {
                    Ok(()) => {}
                    Err(e) => {
                        warn!("HTTP 转发失败 {}:{}: {}", host, port, e);
                    }
                },
            }
        }
        None => {
            client
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
        }
    }

    Ok(())
}

/// GitHub HTML 页面注入脚本，将下载链接重定向到镜像站
fn inject_mirror_script(html: &str) -> String {
    // 安全的镜像脚本，使用 try-catch 防止报错
    const MIRROR_SCRIPT: &str = r#"
<script>
(function(){
try {
    const MIRROR = 'https://v4.gh-proxy.org';
    if (!document.body) return;
    function rewriteLinks() {
        try {
            document.querySelectorAll('a[href*="/releases/download/"]').forEach(function(a) {
                if (a.href && !a.href.includes(MIRROR)) {
                    a.href = MIRROR + '/' + a.href;
                }
            });
            document.querySelectorAll('a[href*="/archive/"]').forEach(function(a) {
                if (a.href && !a.href.includes(MIRROR)) {
                    a.href = MIRROR + '/' + a.href;
                }
            });
        } catch(e) {}
    }
    rewriteLinks();
    if (typeof MutationObserver !== 'undefined') {
        new MutationObserver(rewriteLinks).observe(document.body, {childList: true, subtree: true});
    }
} catch(e) {}
})();
</script>
"#;

    // 安全注入：优先在 </body> 前，如果没有则在末尾
    if html.contains("</body>") {
        html.replace("</body>", &(MIRROR_SCRIPT.to_string() + "</body>"))
    } else if html.contains("</html>") {
        html.replace("</html>", &(MIRROR_SCRIPT.to_string() + "</html>"))
    } else {
        // 没有结束标签，追加到末尾
        html.to_string() + MIRROR_SCRIPT
    }
}

fn is_pac_request(first_line: &str) -> bool {
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    let method = parts[0];
    let path = parts[1];

    method == "GET" && (path == "/pac" || path.ends_with("/pac"))
}

async fn forward_http(
    mut client: TcpStream,
    request_str: &str,
    host: &str,
    port: u16,
    ctx: &HandlerContext,
) -> anyhow::Result<()> {
    use tokio::io::copy_bidirectional;

    let ip = ctx.tls_origin.resolve(host).await?;
    let addr = std::net::SocketAddr::new(ip, port);

    info!("HTTP 转发: 解析 {} -> {}", host, addr);

    let mut target = TcpStream::connect(addr).await?;
    target.set_nodelay(true)?;

    let rewritten = rewrite_http_request(request_str);

    target.write_all(rewritten.as_bytes()).await?;

    let timeout = Duration::from_secs(BIDIRECTIONAL_TIMEOUT_SECS);
    match tokio::time::timeout(timeout, copy_bidirectional(&mut client, &mut target)).await {
        Ok(Ok((c2s, s2c))) => {
            ctx.metrics.add_bytes_sent(c2s);
            ctx.metrics.add_bytes_received(s2c);
        }
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => anyhow::bail!(
            "HTTP 转发传输超时 {} ({}s)",
            host,
            BIDIRECTIONAL_TIMEOUT_SECS
        ),
    }

    Ok(())
}

fn rewrite_http_request(request: &str) -> String {
    let header_end = request.find("\r\n\r\n");
    let (header_part, body_part) = match header_end {
        Some(pos) => (&request[..pos], &request[pos + 4..]),
        None => (request, ""),
    };

    let mut lines = header_part.lines();
    let first_line = match lines.next() {
        Some(l) => l,
        None => return request.to_string(),
    };

    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 3 {
        return request.to_string();
    }

    let method = parts[0];
    let url = parts[1];
    let version = parts[2];

    let path = if let Some(idx) = url.find("//") {
        let after_scheme = &url[idx + 2..];
        if let Some(slash_idx) = after_scheme.find('/') {
            &after_scheme[slash_idx..]
        } else {
            "/"
        }
    } else {
        url
    };

    let mut result = format!("{} {} {}\r\n", method, path, version);

    for line in lines {
        let lower = line.to_lowercase();
        if lower.starts_with("proxy-") {
            continue;
        }
        result.push_str(line);
        result.push_str("\r\n");
    }

    result.push_str("\r\n");

    if !body_part.is_empty() {
        result.push_str(body_part);
    }

    result
}

fn parse_request_path(line: &str) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        Some(parts[1].to_string())
    } else {
        None
    }
}

// 镜像站请求改写：/path → /https://original_host/path
fn rewrite_request_for_mirror(
    original_data: &[u8],
    mirror_host: &str,
    original_host: &str,
) -> Vec<u8> {
    let sep = b"\r\n\r\n";
    let header_end = original_data
        .windows(4)
        .position(|w| w == sep)
        .unwrap_or(original_data.len());
    let body = if header_end + 4 < original_data.len() {
        &original_data[header_end + 4..]
    } else {
        b""
    };

    let header_str = String::from_utf8_lossy(&original_data[..header_end]);
    let mut result = String::with_capacity(
        header_str.len() + mirror_host.len() + original_host.len() + body.len() + 128,
    );

    for (i, line) in header_str.lines().enumerate() {
        if i == 0 {
            // 改写请求行：相对路径 → 镜像站格式 (/https://original_host/path)
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let method = parts[0];
                let path = parts[1];
                let version = parts[2];

                if path.starts_with('/') {
                    // 相对路径 → 镜像站格式: /https://original_host/path
                    result.push_str(&format!(
                        "{} /https://{}{} {}\r\n",
                        method, original_host, path, version
                    ));
                } else if path.starts_with("http://") || path.starts_with("https://") {
                    // 已经是完整 URL，转换为镜像站格式: /https://...
                    result.push_str(&format!("{} /{} {}\r\n", method, path, version));
                } else {
                    // 非标准路径（如 * 或 CONNECT 目标），保持原样
                    result.push_str(line);
                    result.push_str("\r\n");
                }
            } else if parts.len() == 2 {
                // 缺少 HTTP 版本，补全路径 + 默认版本
                let method = parts[0];
                let path = parts[1];
                if path.starts_with('/') {
                    result.push_str(&format!(
                        "{} /https://{}{} HTTP/1.1\r\n",
                        method, original_host, path
                    ));
                } else {
                    result.push_str(line);
                    result.push_str("\r\n");
                }
            } else {
                // 无法解析，保持原样
                result.push_str(line);
                result.push_str("\r\n");
            }
        } else if let Some(pos) = line.find(':') {
            let header_name = line[..pos].trim().to_lowercase();
            if header_name == "host" {
                result.push_str(&format!("Host: {}\r\n", mirror_host));
                continue;
            }
            result.push_str(line);
            result.push_str("\r\n");
        } else {
            result.push_str(line);
            result.push_str("\r\n");
        }
    }

    result.push_str("\r\n");
    let mut result_bytes = result.into_bytes();
    result_bytes.extend_from_slice(body);

    result_bytes
}

fn build_redirect_response(target_url: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        target_url
    ).into_bytes()
}

fn parse_connect_target(line: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "CONNECT" {
        return None;
    }

    let target = parts[1];
    let mut split = target.rsplitn(2, ':');
    let port: u16 = split.next()?.parse().ok()?;
    let host = split.next()?.to_string();

    if host.is_empty() {
        return None;
    }

    Some((host, port))
}

fn parse_http_target(line: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let url = parts[1];
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }

    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_port = without_scheme.split('/').next()?;

    let (host, port) = if host_port.contains(':') {
        let mut split = host_port.rsplitn(2, ':');
        let port: u16 = split.next()?.parse().ok()?;
        let host = split.next()?.to_string();
        (host, port)
    } else if url.starts_with("https://") {
        (host_port.to_string(), 443)
    } else {
        (host_port.to_string(), 80)
    };

    Some((host, port))
}

fn contains_header_end(buf: &[u8]) -> bool {
    windows_ext::find_bytes(buf, b"\r\n\r\n").is_some()
}

mod windows_ext {
    pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}

/// 从缓存构建 HTTP 响应
fn build_response_from_cache(cached: &crate::cache::CachedResource) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        cached.content_type,
        cached.data.len()
    );

    if let Some(ref etag) = cached.etag {
        response.push_str(&format!("ETag: {}\r\n", etag));
    }

    if let Some(ref lm) = cached.last_modified {
        response.push_str(&format!("Last-Modified: {}\r\n", lm));
    }

    response.push_str("Connection: close\r\n\r\n");

    let mut response_bytes = response.into_bytes();
    response_bytes.extend_from_slice(&cached.data);
    response_bytes
}

/// 查找响应头结束位置
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 解析 Content-Length
fn parse_content_length(header: &[u8]) -> Option<u64> {
    let header_str = String::from_utf8_lossy(header);
    for line in header_str.lines().skip(1) {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("content-length:") {
            return line[15..].trim().parse().ok();
        }
    }
    None
}

/// 解析响应头，提取 Content-Type 和 Content-Length
fn parse_response_headers(response: &[u8]) -> (Option<String>, Option<u64>) {
    let header_end = match find_header_end(response) {
        Some(pos) => pos,
        None => return (None, None),
    };

    let header_str = String::from_utf8_lossy(&response[..header_end]);
    let mut content_type = None;
    let mut content_length = None;

    for line in header_str.lines().skip(1) {
        // 跳过状态行
        let line_lower = line.to_lowercase();

        if line_lower.starts_with("content-type:") {
            content_type = Some(line[13..].trim().to_string());
        } else if line_lower.starts_with("content-length:") {
            content_length = line[15..].trim().parse().ok();
        }
    }

    (content_type, content_length)
}
