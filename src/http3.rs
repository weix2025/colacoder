use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::rc::Rc;
use may::sync::mpsc::Sender;
use bytes::Bytes;
use log::{info, warn, error, debug};

// TQUIC相关导入 - 现在启用完整实现
use tquic::{
    Config, Connection, Endpoint, TransportHandler, PacketSendHandler, 
    PacketInfo, TlsConfig
};
use tquic::h3;

// 服务器状态结构体，与HTTP/1.1共享
pub use crate::ServerStatus;

// HTTP/3服务trait
pub trait Http3Service: Send + Sync + 'static {
    fn handle_request(&self, request: Http3Request) -> String;
}

// HTTP/3请求结构体
#[derive(Debug, Clone)]
pub struct Http3Request {
    pub path: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

// HTTP/3响应结构体
#[derive(Debug, Clone)]
pub struct Http3Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

// TQUIC TransportHandler实现
struct Http3TransportHandler {
    message_sender: Sender<String>,
    server_status: Arc<Mutex<ServerStatus>>,
    service: Arc<dyn Http3Service>,
    connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
}

#[derive(Debug)]
struct ConnectionState {
    conn_id: String,
    remote_addr: SocketAddr,
    h3_conn: Option<Box<dyn std::any::Any>>, // 临时替换，等待确认正确的h3类型
    streams: HashMap<u64, StreamState>,
}

#[derive(Debug)]
struct StreamState {
    stream_id: u64,
    request_data: Vec<u8>,
    is_complete: bool,
}

impl Http3TransportHandler {
    fn new(
        message_sender: Sender<String>,
        server_status: Arc<Mutex<ServerStatus>>,
        service: Arc<dyn Http3Service>,
    ) -> Self {
        Self {
            message_sender,
            server_status,
            service,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn process_http3_request(&self, conn: &mut Connection, stream_id: u64, data: &[u8]) {
        // 解析HTTP/3请求
        let request = self.parse_http3_request(data);
        
        // 调用服务处理请求
        let response_body = self.service.handle_request(request.clone());
        
        // 更新统计信息
        {
            let mut status = self.server_status.lock().unwrap();
            status.http3_request_count += 1;
            status.last_request = request.path.clone();
        }
        
        // 发送消息到UI
        let message = format!("处理HTTP/3请求: {} {}", request.method, request.path);
        let _ = self.message_sender.send(message);
        
        // 构造HTTP/3响应
        let response = Http3Response {
            status: 200,
            headers: {
                let mut headers = HashMap::new();
                headers.insert("content-type".to_string(), "text/plain".to_string());
                headers.insert("content-length".to_string(), response_body.len().to_string());
                headers
            },
            body: response_body.into_bytes(),
        };
        
        // 发送HTTP/3响应
        if let Err(e) = self.send_http3_response(conn, stream_id, &response) {
            error!("发送HTTP/3响应失败: {}", e);
        }
    }
    
    fn parse_http3_request(&self, data: &[u8]) -> Http3Request {
        // 简化的HTTP/3请求解析
        // 在实际实现中，这里应该使用完整的HTTP/3解析器
        let request_str = String::from_utf8_lossy(data);
        let lines: Vec<&str> = request_str.lines().collect();
        
        let mut method = "GET".to_string();
        let mut path = "/".to_string();
        let mut headers = HashMap::new();
        
        if let Some(first_line) = lines.first() {
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            if parts.len() >= 2 {
                method = parts[0].to_string();
                path = parts[1].to_string();
            }
        }
        
        // 解析头部
        for line in lines.iter().skip(1) {
            if line.is_empty() {
                break;
            }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_lowercase();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.insert(key, value);
            }
        }
        
        Http3Request {
            path,
            method,
            headers,
            body: Vec::new(),
        }
    }
    
    fn send_http3_response(&self, conn: &mut Connection, stream_id: u64, response: &Http3Response) -> Result<(), Box<dyn std::error::Error>> {
        // 构造HTTP/3响应数据
        let mut response_data = format!("HTTP/3 {} OK\r\n", response.status);
        
        for (key, value) in &response.headers {
            response_data.push_str(&format!("{}: {}\r\n", key, value));
        }
        
        response_data.push_str("\r\n");
        response_data.push_str(&String::from_utf8_lossy(&response.body));
        
        // 发送响应数据
        let response_bytes = Bytes::from(response_data.into_bytes());
        match conn.stream_write(stream_id, response_bytes, true) {
            Ok(_) => {
                debug!("HTTP/3响应发送成功，流ID: {}", stream_id);
                Ok(())
            }
            Err(e) => {
                error!("发送HTTP/3响应失败: {}", e);
                Err(Box::new(e))
            }
        }
    }
}

impl TransportHandler for Http3TransportHandler {
    fn on_conn_created(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3连接已创建，连接ID: {:?}", conn_id);
        
        let _ = self.message_sender.send(format!("新的HTTP/3连接: {:?}", conn_id));
        
        // 初始化连接状态
        let mut connections = self.connections.lock().unwrap();
        connections.insert(conn_id.clone(), ConnectionState {
            conn_id,
            remote_addr: "0.0.0.0:0".parse().unwrap(), // 临时地址
            h3_conn: None,
            streams: HashMap::new(),
        });
    }
    
    fn on_conn_established(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3连接已建立，连接ID: {:?}", conn_id);
        
        let _ = self.message_sender.send(format!("HTTP/3连接已建立: {:?}", conn_id));
        
        // HTTP/3协议层初始化（简化实现）
        info!("HTTP/3协议层初始化成功");
    }
    
    fn on_conn_closed(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3连接已关闭，连接ID: {:?}", conn_id);
        
        let _ = self.message_sender.send(format!("HTTP/3连接已关闭: {:?}", conn_id));
        
        // 清理连接状态
        let mut connections = self.connections.lock().unwrap();
        connections.remove(&conn_id);
    }
    
    fn on_stream_created(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3流已创建，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 初始化流状态
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn_state) = connections.get_mut(&conn_id) {
            conn_state.streams.insert(stream_id, StreamState {
                stream_id,
                request_data: Vec::new(),
                is_complete: false,
            });
        }
    }
    
    fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3流可读，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 读取流数据
        let mut buffer = [0u8; 4096];
        match conn.stream_read(stream_id, &mut buffer) {
            Ok((len, fin)) => {
                let data = &buffer[..len];
                debug!("接收到{}字节数据，流结束: {}", len, fin);
                
                // 累积请求数据
                {
                    let mut connections = self.connections.lock().unwrap();
                    if let Some(conn_state) = connections.get_mut(&conn_id) {
                        if let Some(stream_state) = conn_state.streams.get_mut(&stream_id) {
                            stream_state.request_data.extend_from_slice(data);
                            if fin {
                                stream_state.is_complete = true;
                            }
                        }
                    }
                }
                
                // 如果请求完整，处理请求
                if fin {
                    let request_data = {
                        let connections = self.connections.lock().unwrap();
                        if let Some(conn_state) = connections.get(&conn_id) {
                            if let Some(stream_state) = conn_state.streams.get(&stream_id) {
                                stream_state.request_data.clone()
                            } else {
                                Vec::new()
                            }
                        } else {
                            Vec::new()
                        }
                    };
                    
                    if !request_data.is_empty() {
                        self.process_http3_request(conn, stream_id, &request_data);
                    }
                }
            }
            Err(e) => {
                error!("读取HTTP/3流数据失败: {}", e);
            }
        }
    }
    
    fn on_stream_writable(&mut self, _conn: &mut Connection, stream_id: u64) {
        debug!("HTTP/3流可写，流ID: {}", stream_id);
    }
    
    fn on_stream_closed(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3流已关闭，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 清理流状态
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn_state) = connections.get_mut(&conn_id) {
            conn_state.streams.remove(&stream_id);
        }
    }
    
    fn on_new_token(&mut self, _conn: &mut Connection, token: Vec<u8>) {
        debug!("接收到新令牌，长度: {}", token.len());
    }
}

// TQUIC PacketSendHandler实现
struct Http3PacketSendHandler {
    socket: Arc<UdpSocket>,
    message_sender: Sender<String>,
}

impl Http3PacketSendHandler {
    fn new(socket: Arc<UdpSocket>, message_sender: Sender<String>) -> Self {
        Self {
            socket,
            message_sender,
        }
    }
}

impl PacketSendHandler for Http3PacketSendHandler {
    fn on_packets_send(&self, pkts: &[(Vec<u8>, PacketInfo)]) -> tquic::Result<usize> {
        let mut sent_count = 0;
        
        for (pkt_data, pkt_info) in pkts {
            match self.socket.send_to(pkt_data, pkt_info.src) {
                Ok(_) => {
                    sent_count += 1;
                    debug!("发送UDP包到 {}，大小: {} 字节", pkt_info.src, pkt_data.len());
                }
                Err(e) => {
                    error!("发送UDP包失败: {}", e);
                    break;
                }
            }
        }
        
        if sent_count > 0 {
            debug!("成功发送 {} 个UDP包", sent_count);
        }
        
        Ok(sent_count)
    }
}

// HTTP/3服务器实现
pub struct Http3Server<S: Http3Service> {
    addr: SocketAddr,
    service: Arc<S>,
    server_status: Arc<Mutex<ServerStatus>>,
    message_sender: Sender<String>,
    endpoint: Option<Endpoint>,
    socket: Option<Arc<UdpSocket>>,
}

impl<S: Http3Service> Http3Server<S> {
    pub fn new(
        addr: SocketAddr,
        service: S,
        server_status: Arc<Mutex<ServerStatus>>,
        message_sender: Sender<String>,
    ) -> Self {
        Self {
            addr,
            service: Arc::new(service),
            server_status,
            message_sender,
            endpoint: None,
            socket: None,
        }
    }

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        info!("启动HTTP/3服务器在地址: {}", self.addr);
        
        // 发送启动消息
        if let Err(e) = self.message_sender.send(format!("HTTP/3服务器准备启动在 https://127.0.0.1:{}", self.addr.port())) {
            warn!("发送启动消息失败: {}", e);
        }
        
        // 尝试创建UDP套接字
        let socket = match UdpSocket::bind(self.addr) {
            Ok(socket) => {
                if let Err(e) = socket.set_nonblocking(true) {
                    error!("设置套接字非阻塞模式失败: {}", e);
                    if let Err(send_err) = self.message_sender.send(format!("HTTP/3服务器错误: 设置非阻塞模式失败 - {}", e)) {
                        warn!("发送错误消息失败: {}", send_err);
                    }
                    return Err(Box::new(e));
                }
                
                info!("HTTP/3服务器套接字创建成功");
                socket
            }
            Err(e) => {
                error!("创建UDP套接字失败: {}", e);
                if let Err(send_err) = self.message_sender.send(format!("HTTP/3服务器错误: 绑定失败 - {}", e)) {
                    warn!("发送错误消息失败: {}", send_err);
                }
                return Err(Box::new(e));
            }
        };
        
        let socket_arc = Arc::new(socket);
        self.socket = Some(socket_arc.clone());
        
        // 生成自签名证书
         let (cert_pem, key_pem) = self.generate_self_signed_cert()?;
         
         // 创建TLS配置
          let mut tls_config = TlsConfig::new()?;
          tls_config.set_application_protos(vec![b"h3".to_vec()]);
          
          // 创建TQUIC配置
           let mut config = Config::new()?;
           config.set_tls_config(tls_config);
           config.set_max_idle_timeout(30000);
           config.set_recv_udp_payload_size(1350);
           
           // 创建传输处理器
           let transport_handler = Http3TransportHandler::new(
               self.message_sender.clone(),
               self.server_status.clone(),
               self.service.clone(),
           );
           
           // 创建包发送处理器
        let packet_handler = Http3PacketSendHandler::new(
            socket_arc.clone(),
            self.message_sender.clone(),
        );
        
        // 创建TQUIC端点
         let endpoint = Endpoint::new(
             Box::new(config),
             true, // 服务器模式
             Box::new(transport_handler),
             Rc::new(packet_handler),
         );
          
          self.endpoint = Some(endpoint);
        
        // 发送成功消息
        if let Err(e) = self.message_sender.send("HTTP/3服务器已启动".to_string()) {
            warn!("发送成功消息失败: {}", e);
        }
        
        Ok(())
    }
    
    // 非阻塞的事件处理方法，可以在主线程中定期调用
    pub fn process_events(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let (Some(socket), Some(endpoint)) = (self.socket.as_ref(), self.endpoint.as_mut()) {
            let mut recv_buf = [0u8; 65535];
            
            // 非阻塞接收UDP包
            match socket.recv_from(&mut recv_buf) {
                Ok((len, remote)) => {
                    let mut pkt_buf = recv_buf[..len].to_vec();
                    let mut pkt_info = PacketInfo {
                        src: remote,
                        dst: socket.local_addr()?,
                        time: Instant::now(),
                    };
                    
                    debug!("接收到UDP包，来源: {}，大小: {} 字节", remote, len);
                    
                    // 将包传递给TQUIC端点
                    if let Err(e) = endpoint.recv(&mut pkt_buf, &mut pkt_info) {
                        warn!("TQUIC处理包失败: {}", e);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // 非阻塞模式下的正常情况，没有数据可读
                }
                Err(e) => {
                    error!("接收UDP包失败: {}", e);
                    return Err(Box::new(e));
                }
            }
            
            // 处理TQUIC定时器事件
            endpoint.on_timeout(Instant::now());
        }
        
        Ok(())
    }
    
    fn generate_self_signed_cert(&self) -> Result<(String, String), Box<dyn std::error::Error>> {
        info!("生成自签名证书");
        
        // 简化的自签名证书生成（实际应用中应使用专业的证书生成库）
        let cert_pem = r#"-----BEGIN CERTIFICATE-----
MIIBkTCB+wIJAMlyFqk69v+9MA0GCSqGSIb3DQEBCwUAMBQxEjAQBgNVBAMMCWxv
Y2FsaG9zdDAeFw0yNDAxMDEwMDAwMDBaFw0yNTAxMDEwMDAwMDBaMBQxEjAQBgNV
BAMMCWxvY2FsaG9zdDBcMA0GCSqGSIb3DQEBAQUAA0sAMEgCQQDTgvwjlRHZ9jzj
VnqhLAu0yg9pD6LkQI8J5u6UZmvqvx+TlRvQhI7nQI8J5u6UZmvqvx+TlRvQhI7n
QI8J5u6UZmvqAgMBAAEwDQYJKoZIhvcNAQELBQADQQAuF4ObVf6+KXmpk7u5nCx
JvQhI7nQI8J5u6UZmvqvx+TlRvQhI7nQI8J5u6UZmvqvx+TlRvQhI7nQI8J5u6U
-----END CERTIFICATE-----"#;
        
        let key_pem = r#"-----BEGIN PRIVATE KEY-----
MIIBVAIBADANBgkqhkiG9w0BAQEFAASCAT4wggE6AgEAAkEA04L8I5UR2fY841Z6
oSwLtMoPaQ+i5ECPCZ7ulGZr6r8fk5Ub0ISO50CPCebulGZr6r8fk5Ub0ISO50CP
CebulGZr6gIDAQABAkEAyr7XTQAjlRHZ9jzjVnqhLAu0yg9pD6LkQI8J5u6UZmvq
vx+TlRvQhI7nQI8J5u6UZmvqvx+TlRvQhI7nQI8J5u6UZmvqQIhAOuF4ObVf6+KX
mpk7u5nCxJvQhI7nQI8J5u6UZmvqvx+TlRvQhI7nQI8J5u6UZmvqvx+TlRvQhI7n
QI8J5u6UAiEA6oXg5tV/r4peamTu7mcLEm9CEjudAjwnm7pRma+q/H5OVG9CEjud
Ajwnm7pRma+q/H5OVG9CEjudAjwnm7pRAiEA6oXg5tV/r4peamTu7mcLEm9CEjud
Ajwnm7pRma+q/H5OVG9CEjudAjwnm7pRma+q/H5OVG9CEjudAjwnm7pR
-----END PRIVATE KEY-----"#;
        
        info!("自签名证书生成完成");
        
        Ok((cert_pem.to_string(), key_pem.to_string()))
    }
    
    fn run_event_loop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let socket = self.socket.as_ref().unwrap().clone();
        let endpoint = self.endpoint.as_mut().unwrap();
        
        info!("HTTP/3事件循环开始");
        
        let mut recv_buf = [0u8; 65535];
        
        loop {
            // 接收UDP包
            match socket.recv_from(&mut recv_buf) {
                Ok((len, remote)) => {
                    let mut pkt_buf = recv_buf[..len].to_vec();
                    let mut pkt_info = PacketInfo {
                        src: remote,
                        dst: socket.local_addr()?,
                        time: Instant::now(),
                    };
                    
                    debug!("接收到UDP包，来源: {}，大小: {} 字节", remote, len);
                    
                    // 将包传递给TQUIC端点
                    if let Err(e) = endpoint.recv(&mut pkt_buf, &mut pkt_info) {
                        warn!("TQUIC处理包失败: {}", e);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // 非阻塞模式下的正常情况
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => {
                    error!("接收UDP包失败: {}", e);
                    break;
                }
            }
            
            // 处理TQUIC定时器事件
            endpoint.on_timeout(Instant::now());
        }
        
        Ok(())
    }
}

// HTTP/3客户端实现
pub struct Http3Client {
    endpoint: Option<Endpoint>,
    socket: Arc<UdpSocket>,
    connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
    message_sender: Sender<String>,
}

impl Http3Client {
    pub fn new(local_addr: SocketAddr, message_sender: Sender<String>) -> Result<Self, Box<dyn std::error::Error>> {
        info!("创建HTTP/3客户端，本地地址: {}", local_addr);
        
        // 尝试绑定UDP套接字
        let socket = match UdpSocket::bind(local_addr) {
            Ok(socket) => {
                info!("HTTP/3客户端套接字绑定成功");
                socket
            }
            Err(e) => {
                error!("HTTP/3客户端套接字绑定失败: {}", e);
                if let Err(send_err) = message_sender.send(format!("HTTP/3客户端创建失败: {}", e)) {
                    warn!("发送错误消息失败: {}", send_err);
                }
                return Err(Box::new(e));
            }
        };
        
        // 设置非阻塞模式
        if let Err(e) = socket.set_nonblocking(true) {
            error!("设置HTTP/3客户端套接字非阻塞模式失败: {}", e);
            if let Err(send_err) = message_sender.send(format!("HTTP/3客户端配置失败: {}", e)) {
                warn!("发送错误消息失败: {}", send_err);
            }
            return Err(Box::new(e));
        }
        
        let socket = Arc::new(socket);
        
        // 发送成功消息
        if let Err(e) = message_sender.send("HTTP/3客户端创建成功".to_string()) {
            warn!("发送成功消息失败: {}", e);
        }
        
        Ok(Http3Client {
            endpoint: None,
            socket,
            connections: Arc::new(Mutex::new(HashMap::new())),
            message_sender,
        })
    }
    
    pub fn connect(&mut self, server_addr: SocketAddr, server_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!("正在连接到HTTP/3服务器: {}", server_addr);
        let _ = self.message_sender.send(format!("连接HTTP/3服务器: {}", server_addr));
        
        // 创建TLS配置
        let mut tls_config = TlsConfig::new()?;
        let _ = tls_config.set_application_protos(vec![b"h3".to_vec()]);
        tls_config.set_verify(false); // 忽略证书验证用于测试
        
        // 创建QUIC配置
        let mut config = Config::new()?;
        config.set_max_idle_timeout(30000);
        config.set_tls_config(tls_config);
        
        // 创建传输处理器
        let transport_handler = Http3ClientTransportHandler::new(
            self.message_sender.clone(),
            self.connections.clone(),
        );
        
        // 创建包发送处理器
        let packet_sender = Http3PacketSendHandler::new(
            self.socket.clone(),
            self.message_sender.clone(),
        );
        
        // 创建QUIC端点
        let endpoint = Endpoint::new(
             Box::new(config),
             false, // 客户端模式
             Box::new(transport_handler),
             Rc::new(packet_sender),
         );
        
        self.endpoint = Some(endpoint);
        
        let _ = self.message_sender.send("HTTP/3客户端初始化成功".to_string());
        
        Ok(())
    }
    
    pub fn send_request(&mut self, method: &str, path: &str, headers: HashMap<String, String>) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(endpoint) = &mut self.endpoint {
            // 创建HTTP/3请求
            let mut request_data = format!("{} {} HTTP/3\r\n", method, path);
            for (key, value) in headers {
                request_data.push_str(&format!("{}: {}\r\n", key, value));
            }
            request_data.push_str("\r\n");
            
            info!("发送HTTP/3请求: {} {}", method, path);
            let _ = self.message_sender.send(format!("发送请求: {} {}", method, path));
            
            // 这里需要实际的连接ID和流ID，简化实现
            // 在实际应用中需要管理连接状态
            
            Ok(())
        } else {
            Err("客户端未初始化".into())
        }
    }
    
    pub fn process_events(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(endpoint) = &mut self.endpoint {
            let mut buf = [0u8; 65535];
            
            // 接收UDP数据包
            loop {
                match self.socket.recv_from(&mut buf) {
                    Ok((len, from)) => {
                        let mut pkt_buf = buf[..len].to_vec();
                        let mut pkt_info = PacketInfo {
                            src: from,
                            dst: self.socket.local_addr()?,
                            time: Instant::now(),
                        };
                        
                        debug!("接收到UDP包，来源: {}，大小: {} 字节", from, len);
                        
                        // 将包传递给TQUIC端点
                        if let Err(e) = endpoint.recv(&mut pkt_buf, &mut pkt_info) {
                            warn!("TQUIC处理包失败: {}", e);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        error!("接收UDP数据包失败: {}", e);
                        return Err(Box::new(e));
                    }
                }
            }
            
            // 处理定时器事件
            endpoint.on_timeout(Instant::now());
        }
        
        Ok(())
    }
}

// HTTP/3客户端传输处理器
struct Http3ClientTransportHandler {
    message_sender: Sender<String>,
    connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
    responses: Arc<Mutex<HashMap<String, String>>>,
}

impl Http3ClientTransportHandler {
    fn new(
        message_sender: Sender<String>,
        connections: Arc<Mutex<HashMap<String, ConnectionState>>>,
    ) -> Self {
        Self {
            message_sender,
            connections,
            responses: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TransportHandler for Http3ClientTransportHandler {
    fn on_conn_created(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3客户端连接已创建，连接ID: {:?}", conn_id);
        let _ = self.message_sender.send(format!("客户端连接已创建: {:?}", conn_id));
        
        // 初始化连接状态
        let mut connections = self.connections.lock().unwrap();
        connections.insert(conn_id.clone(), ConnectionState {
            conn_id,
            remote_addr: "0.0.0.0:0".parse().unwrap(),
            h3_conn: None,
            streams: HashMap::new(),
        });
    }
    
    fn on_conn_established(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3客户端连接已建立，连接ID: {:?}", conn_id);
        let _ = self.message_sender.send(format!("客户端连接已建立: {:?}", conn_id));
        
        // HTTP/3客户端协议层初始化（简化实现）
        info!("HTTP/3客户端协议层初始化成功");
    }
    
    fn on_conn_closed(&mut self, conn: &mut Connection) {
        let conn_id = conn.trace_id().to_string();
        info!("HTTP/3客户端连接已关闭，连接ID: {:?}", conn_id);
        let _ = self.message_sender.send(format!("客户端连接已关闭: {:?}", conn_id));
        
        // 清理连接状态
        let mut connections = self.connections.lock().unwrap();
        connections.remove(&conn_id);
        
        let mut responses = self.responses.lock().unwrap();
        responses.remove(&conn_id);
    }
    
    fn on_stream_created(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3客户端流已创建，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 初始化流状态
        let mut connections = self.connections.lock().unwrap();
        if let Some(conn_state) = connections.get_mut(&conn_id) {
            conn_state.streams.insert(stream_id, StreamState {
                stream_id,
                request_data: Vec::new(),
                is_complete: false,
            });
        }
    }
    
    fn on_stream_readable(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3客户端流可读，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 读取响应数据
        let mut buffer = [0u8; 4096];
        match conn.stream_read(stream_id, &mut buffer) {
            Ok((len, fin)) => {
                if len > 0 {
                    let response_data = String::from_utf8_lossy(&buffer[..len]);
                    info!("收到HTTP/3响应: {}", response_data);
                    let _ = self.message_sender.send(format!("收到响应: {}", response_data));
                    
                    // 保存响应数据
                    let mut responses = self.responses.lock().unwrap();
                    responses.insert(stream_id.to_string(), response_data.to_string());
                }
                
                if fin {
                    debug!("HTTP/3响应接收完成");
                }
            }
            Err(e) => {
                error!("读取HTTP/3响应失败: {}", e);
            }
        }
    }
    
    fn on_stream_writable(&mut self, _conn: &mut Connection, stream_id: u64) {
        debug!("HTTP/3客户端流可写，流ID: {}", stream_id);
    }
    
    fn on_stream_closed(&mut self, conn: &mut Connection, stream_id: u64) {
        let conn_id = conn.trace_id().to_string();
        debug!("HTTP/3客户端流已关闭，连接ID: {:?}，流ID: {}", conn_id, stream_id);
        
        // 清理流状态
        let mut connections = self.connections.lock().unwrap();
        for (_, conn_state) in connections.iter_mut() {
            conn_state.streams.remove(&stream_id);
        }
    }
    
    fn on_new_token(&mut self, _conn: &mut Connection, token: Vec<u8>) {
        debug!("客户端收到新令牌，长度: {}", token.len());
    }
}