// src/http3.rs

use log::*;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;

use thiserror::Error;
use tquic::connection::Connection;
use tquic::endpoint::Endpoint;
use tquic::h3::{Header, NameValue, Http3Config};
use tquic::h3::connection::Http3Connection;
use tquic::Config;
use bytes::Bytes;
use may::coroutine;
use tquic::{TransportHandler, PacketSendHandler, PacketInfo};
use std::rc::Rc;

// 重新导出必要的类型
pub use tquic::Result;

/// 企业级HTTP/3错误类型
#[derive(Error, Debug)]
pub enum TquicError {
    #[error("TQUIC错误: {0}")]
    Tquic(#[from] tquic::Error),
    #[error("IO错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("证书错误: {0}")]
    Certificate(String),
    #[error("连接错误: {0}")]
    Connection(String),
    #[error("HTTP/3错误: {0}")]
    Http3(String),
}

/// 企业级HTTP/3请求结构体
#[derive(Debug, Clone)]
pub struct Http3Request {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Bytes>,
    pub timestamp: std::time::SystemTime,
}

impl Http3Request {
    pub fn new(method: String, path: String) -> Self {
        Self {
            method,
            scheme: "https".to_string(),
            authority: "localhost".to_string(),
            path,
            headers: HashMap::new(),
            body: None,
            timestamp: std::time::SystemTime::now(),
        }
    }
    
    pub fn with_header(mut self, name: String, value: String) -> Self {
        self.headers.insert(name, value);
        self
    }
    
    pub fn with_body(mut self, body: Bytes) -> Self {
        self.body = Some(body);
        self
    }
}

/// 企业级HTTP/3响应结构体
#[derive(Debug, Clone)]
pub struct Http3Response {
    pub status: u64,
    pub headers: HashMap<String, String>,
    pub body: Option<Bytes>,
    pub timestamp: std::time::SystemTime,
}

impl Http3Response {
    pub fn new(status: u64) -> Self {
        let mut headers = HashMap::new();
        headers.insert("server".to_string(), "Enterprise-HTTP3-Server/1.0".to_string());
        headers.insert("date".to_string(), chrono::Utc::now().to_rfc2822());
        
        Self {
            status,
            headers,
            body: None,
            timestamp: std::time::SystemTime::now(),
        }
    }
    
    pub fn with_header(mut self, name: String, value: String) -> Self {
        self.headers.insert(name, value);
        self
    }
    
    pub fn with_body(mut self, body: Bytes) -> Self {
        self.headers.insert("content-length".to_string(), body.len().to_string());
        self.body = Some(body);
        self
    }
    
    pub fn json(status: u64, data: &str) -> Self {
        Self::new(status)
            .with_header("content-type".to_string(), "application/json".to_string())
            .with_body(Bytes::from(data.to_string()))
    }
    
    pub fn text(mut self, data: &str) -> Self {
        self.headers.insert("content-type".to_string(), "text/plain; charset=utf-8".to_string());
        self.body = Some(Bytes::from(data.to_string()));
        self
    }
}

/// 简单的传输处理器
struct SimpleTransportHandler;

impl TransportHandler for SimpleTransportHandler {
    fn on_conn_created(&mut self, conn: &mut Connection) {
        // 连接创建时的处理
    }
    
    fn on_conn_established(&mut self, conn: &mut Connection) {
        // 连接建立时的处理
    }
    
    fn on_conn_closed(&mut self, conn: &mut Connection) {
        // 连接关闭时的处理
    }
    
    fn on_stream_created(&mut self, conn: &mut Connection, stream_id: u64) {
        // 流创建时的处理
    }
    
    fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
        // 流可读时的处理
    }
    
    fn on_stream_writable(&mut self, conn: &mut Connection, stream_id: u64) {
        // 流可写时的处理
    }
    
    fn on_stream_closed(&mut self, conn: &mut Connection, stream_id: u64) {
        // 流关闭时的处理
    }
    
    fn on_new_token(&mut self, conn: &mut Connection, token: Vec<u8>) {
        // 新令牌处理
    }
}

/// 简单的数据包发送器
struct SimplePacketSender;

impl PacketSendHandler for SimplePacketSender {
    fn send_packet(&self, packet: &[u8], peer_addr: SocketAddr) -> std::result::Result<(), std::io::Error> {
        // 实际的数据包发送逻辑
        // 这里需要根据实际的网络接口来实现
        Ok(())
    }
}

/// 企业级HTTP/3事件处理器 Trait
pub trait Http3EventHandler: Send + Sync {
    /// 处理HTTP/3请求
    fn on_request(&self, request: Http3Request) -> Http3Response;
    
    /// 连接建立事件
    fn on_connection_established(&self);
    
    /// 连接关闭事件
    fn on_connection_closed(&self);
    
    /// 错误处理事件
    fn on_error(&self, error: String);
    
    /// 可选：处理响应（用于客户端）
    fn on_response(&self, response: Http3Response) {
        // 默认实现为空
    }
}

/// 企业级HTTP/3服务接口
pub trait EnterpriseHttp3 {
    /// 创建新的HTTP/3服务实例
    fn new(addr: SocketAddr, handler: Arc<dyn Http3EventHandler>) -> std::result::Result<Self, TquicError>
    where
        Self: Sized;
    
    /// 启动服务
    fn start(&mut self) -> std::result::Result<(), TquicError>;
    
    /// 停止服务
    fn stop(&mut self);
    
    /// 发送请求（客户端使用）
    fn send_request(&self, request: Http3Request) -> std::result::Result<(), TquicError>;
    
    /// 获取服务地址
    fn local_addr(&self) -> Option<SocketAddr>;
    
    /// 检查服务是否运行中
    fn is_running(&self) -> bool;
}

/// 企业级HTTP/3服务器
pub struct EnterpriseHttp3Server {
    addr: SocketAddr,
    endpoint: Option<Endpoint>,
    handler: Arc<dyn Http3EventHandler>,
    running: Arc<AtomicBool>,
}

impl EnterpriseHttp3Server {
    /// 生成临时证书
    fn generate_temp_certificate() -> std::result::Result<(), TquicError> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .map_err(|e| TquicError::Certificate(e.to_string()))?;
        let cert_der = cert.cert.der();
        let priv_key = cert.signing_key.serialize_der();
        
        fs::write("temp_cert.der", &cert_der)
            .map_err(|e| TquicError::Io(e))?;
        fs::write("temp_key.der", &priv_key)
            .map_err(|e| TquicError::Io(e))?;
        
        info!("临时证书已生成: temp_cert.der, temp_key.der");
        Ok(())
    }
}

impl EnterpriseHttp3 for EnterpriseHttp3Server {
    fn new(addr: SocketAddr, handler: Arc<dyn Http3EventHandler>) -> std::result::Result<Self, TquicError> {
        Ok(Self {
            addr,
            endpoint: None,
            handler,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    fn start(&mut self) -> std::result::Result<(), TquicError> {
        // 生成临时证书
        Self::generate_temp_certificate()?;
        
        let config = create_server_config()?;
        // Create a simple transport handler and packet sender for the endpoint
        let handler = Box::new(SimpleTransportHandler);
        let sender = Rc::new(SimplePacketSender);
        let endpoint = Endpoint::new(Box::new(config), true, handler, sender)
            .map_err(|e| TquicError::Tquic(e))?;
        endpoint.listen(&self.addr)
            .map_err(|e| TquicError::Tquic(e))?;
        
        let local_addr = endpoint.local_addr()
            .map_err(|e| TquicError::Tquic(e))?;
        info!("HTTP/3服务器启动在: {}", local_addr);
        
        self.endpoint = Some(endpoint);
        self.running.store(true, Ordering::SeqCst);
        
        let endpoint_clone = self.endpoint.as_ref().unwrap().clone();
        let handler = self.handler.clone();
        let running = self.running.clone();

        may::go!(move || {
            while running.load(Ordering::SeqCst) {
                if let Some(conn) = endpoint_clone.accept() {
                    let peer_addr = match conn.peer_addr() {
                        Ok(addr) => addr,
                        Err(e) => {
                            error!("获取对等地址失败: {:?}", e);
                            continue;
                        }
                    };
                    
                    info!("新连接来自: {}", peer_addr);
                    handler.on_connection_established();
                    
                    let handler_clone = handler.clone();
                    may::go!(move || {
                        if let Err(e) = handle_connection(conn, handler_clone.clone(), peer_addr) {
                            error!("连接处理失败: {:?}", e);
                            handler_clone.on_error(e.to_string());
                        }
                        handler_clone.on_connection_closed();
                    });
                }
            }
        });
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        info!("HTTP/3服务器已停止");
    }

    fn send_request(&self, _request: Http3Request) -> std::result::Result<(), TquicError> {
        // 服务器不发送请求
        Err(TquicError::Config("服务器不支持发送请求".to_string()))
    }
    
    fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.addr)
    }
    
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// 企业级HTTP/3客户端
pub struct EnterpriseHttp3Client {
    remote_addr: SocketAddr,
    endpoint: Endpoint,
    connection: Option<Connection>,
    handler: Arc<dyn Http3EventHandler>,
    running: Arc<AtomicBool>,
}

impl EnterpriseHttp3 for EnterpriseHttp3Client {
    fn new(addr: SocketAddr, handler: Arc<dyn Http3EventHandler>) -> std::result::Result<Self, TquicError> {
        let config = create_client_config()?;
        let endpoint = Endpoint::new(config, false)
            .map_err(|e| TquicError::Tquic(e))?;

        Ok(Self {
            remote_addr: addr,
            endpoint,
            connection: None,
            handler,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    fn start(&mut self) -> std::result::Result<(), TquicError> {
        let connecting = self.endpoint.connect(&self.remote_addr, "localhost")
            .map_err(|e| TquicError::Tquic(e))?;
        
        let conn = may::coroutine::scope(|s| {
            s.spawn(move || {
                connecting.wait()
            }).join().unwrap()
        }).map_err(|e| TquicError::Tquic(e))?;
        
        let peer_addr = conn.peer_addr().map_err(|e| TquicError::Tquic(e))?;
        info!("已连接到: {}", peer_addr);
        
        self.handler.on_connection_established();
        self.connection = Some(conn);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(mut conn) = self.connection.take() {
            self.handler.on_connection_closed();
            conn.close(false, 0, b"client_shutdown").ok();
        }
        info!("HTTP/3客户端已停止");
    }

    fn send_request(&self, request: Http3Request) -> std::result::Result<(), TquicError> {
        if let Some(conn) = &self.connection {
            let mut conn_mut = unsafe { &mut *(conn as *const Connection as *mut Connection) };
            
            // 创建HTTP/3连接
            let h3_config = Http3Config::new().map_err(|e| TquicError::Http3(format!("创建HTTP/3配置失败: {:?}", e)))?;
            let mut h3_conn = Http3Connection::new_with_quic_conn(conn_mut, &h3_config)
                .map_err(|e| TquicError::Http3(format!("创建HTTP/3连接失败: {:?}", e)))?;
            
            // 创建新的流
            let stream_id = h3_conn.stream_new(conn_mut)
                .map_err(|e| TquicError::Http3(format!("创建流失败: {:?}", e)))?;

            // 构建HTTP/3头部
            let mut headers = vec![
                Header::new(b":method", request.method.as_bytes()),
                Header::new(b":scheme", request.scheme.as_bytes()),
                Header::new(b":authority", request.authority.as_bytes()),
                Header::new(b":path", request.path.as_bytes()),
            ];
            
            // 添加自定义头部
            for (name, value) in &request.headers {
                headers.push(Header::new(name.as_bytes(), value.as_bytes()));
            }
            
            // 发送头部
            h3_conn.send_headers(conn_mut, stream_id, &headers, request.body.is_none())
                .map_err(|e| TquicError::Http3(format!("发送头部失败: {:?}", e)))?;
            
            // 发送请求体（如果有）
            if let Some(body) = &request.body {
                h3_conn.send_body(conn_mut, stream_id, body.clone(), true)
                    .map_err(|e| TquicError::Http3(format!("发送请求体失败: {:?}", e)))?;
            }
                
            info!("HTTP/3请求已发送: {} {}", request.method, request.path);
        } else {
            return Err(TquicError::Config("客户端未连接".to_string()));
        }
        Ok(())
    }
    
    fn local_addr(&self) -> Option<SocketAddr> {
        // 客户端通常没有固定的本地地址，返回None或者从连接中获取
        None
    }
    
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}


fn handle_connection(mut conn: Connection, handler: Arc<dyn Http3EventHandler>, peer_addr: SocketAddr) -> std::result::Result<(), TquicError> {
    info!("处理来自 {} 的HTTP/3连接", peer_addr);
    
    // 通知连接建立
    handler.on_connection_established();
    
    // 创建HTTP/3连接
    let h3_config = Http3Config::new().map_err(|e| TquicError::Http3(format!("创建HTTP/3配置失败: {:?}", e)))?;
    let mut h3_conn = Http3Connection::new_with_quic_conn(&mut conn, &h3_config)
        .map_err(|e| TquicError::Http3(format!("创建HTTP/3连接失败: {:?}", e)))?;
    
    // 简化的连接处理逻辑
    // 注意: 实际的事件处理应该通过Endpoint的事件循环来完成
    // 这里提供一个基本的框架
    
    loop {
        // 检查连接是否已关闭
        if conn.is_closed() {
            info!("连接已关闭");
            break;
        }
        
        // 处理HTTP/3事件
        match h3_conn.poll(&mut conn) {
            Ok((stream_id, event)) => {
                match event {
                    tquic::h3::Http3Event::Headers { headers, .. } => {
                        let request = parse_request_headers(headers);
                        let response = handler.on_request(request);
                        
                        // 发送响应
                        let response_headers = vec![
                            Header::new(b":status", response.status.to_string().as_bytes()),
                            Header::new(b"content-type", b"application/json"),
                        ];
                        
                        if let Err(e) = h3_conn.send_headers(&mut conn, stream_id, &response_headers, false) {
                            error!("发送响应头失败: {:?}", e);
                        }
                        
                        if let Some(body) = response.body {
                            if let Err(e) = h3_conn.send_body(&mut conn, stream_id, body, true) {
                                error!("发送响应体失败: {:?}", e);
                            }
                        }
                    },
                    tquic::h3::Http3Event::Data { .. } => {
                        // 处理数据
                    },
                    tquic::h3::Http3Event::Finished { .. } => {
                        // 流结束
                    },
                    _ => {}
                }
            },
            Err(tquic::h3::Http3Error::Done) => {
                // 没有更多事件，继续循环
                std::thread::sleep(Duration::from_millis(1));
            },
            Err(e) => {
                error!("HTTP/3事件处理错误: {:?}", e);
                handler.on_error(format!("HTTP/3事件处理错误: {:?}", e));
                break;
            }
        }
    }
    
    // 通知连接关闭
    handler.on_connection_closed();
    Ok(())
}

fn parse_request_headers(headers: Vec<Header>) -> Http3Request {
    let mut req = Http3Request {
        method: String::new(),
        scheme: String::new(),
        authority: String::new(),
        path: String::new(),
        headers: HashMap::new(),
        body: None,
        timestamp: std::time::SystemTime::now(),
    };

    for header in &headers {
        match header.name() {
            b":method" => req.method = String::from_utf8_lossy(header.value()).to_string(),
            b":scheme" => req.scheme = String::from_utf8_lossy(header.value()).to_string(),
            b":authority" => req.authority = String::from_utf8_lossy(header.value()).to_string(),
            b":path" => req.path = String::from_utf8_lossy(header.value()).to_string(),
            _ => {
                req.headers.insert(
                    String::from_utf8_lossy(header.name()).to_string(),
                    String::from_utf8_lossy(header.value()).to_string(),
                );
            }
        }
    }
    req
}


/// 创建服务器配置
fn create_server_config() -> std::result::Result<Config, TquicError> {
    let mut config = Config::new().map_err(|e| TquicError::Tquic(e))?;
    
    // 创建TLS配置
    let tls_config = tquic::TlsConfig::new_server_config(
        "temp_cert.der",
        "temp_key.der", 
        vec![b"h3".to_vec()],
        false
    ).map_err(|e| TquicError::Tquic(e))?;
    
    config.set_tls_config(tls_config);
    config.set_max_idle_timeout(30_000);
    config.set_recv_udp_payload_size(1350);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);

    Ok(config)
}

/// 创建客户端配置
fn create_client_config() -> std::result::Result<Config, TquicError> {
    let mut config = Config::new().map_err(|e| TquicError::Tquic(e))?;
    
    // 创建TLS配置
    let mut tls_config = tquic::TlsConfig::new_client_config(
        vec![b"h3".to_vec()],
        false
    ).map_err(|e| TquicError::Tquic(e))?;
    
    tls_config.set_verify(false); // 在生产环境中应该验证服务器证书
    config.set_tls_config(tls_config);
    config.set_max_idle_timeout(30_000);
    config.set_recv_udp_payload_size(1350);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);

    Ok(config)
}

/// 生成临时证书的公共函数
pub fn generate_temp_certificate() -> std::result::Result<(), TquicError> {
    EnterpriseHttp3Server::generate_temp_certificate()
}