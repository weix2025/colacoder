// src/http3.rs

use log::*;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use tquic::connection::Connection;
use tquic::endpoint::{Connecting, Endpoint};
use tquic::h3::{NameValue, Request, Response}; // FIX: Import NameValue trait
use tquic::{Config, Result};

// FIX: Re-export necessary types for main.rs to use
pub use tquic::h3::{Header, Result as Http3Result};

#[derive(Error, Debug)]
pub enum Http3Error {
    #[error("tquic error: {0}")]
    Tquic(#[from] tquic::Error),
    #[error("http3 error: {0}")]
    Http3(#[from] tquic::h3::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("address resolution error: {0}")]
    Addr(String),
}

/// HTTP/3 请求结构体
#[derive(Debug, Clone)]
pub struct Http3Request {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// HTTP/3 响应结构体
#[derive(Debug, Clone)]
pub struct Http3Response {
    pub status: u64,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl Http3Response {
    pub fn new(status: u64, headers: Vec<(String, String)>, body: Option<Vec<u8>>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }
}

/// HTTP/3 事件处理器 Trait
pub trait Http3EventHandler: Send + Sync {
    fn handle_request(&self, request: Http3Request) -> Http3Result<Http3Response>;
    fn handle_response(&self, response: Http3Response);
}

// EnterpriseHttp3 Trait
pub trait EnterpriseHttp3 {
    fn new(addr: &str, handler: Arc<dyn Http3EventHandler>) -> Result<Self>
    where
        Self: Sized;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self);
    fn send_request(&self, request: Http3Request) -> Result<()>;
}

/// EnterpriseHttp3Server 结构体
pub struct EnterpriseHttp3Server {
    addr: SocketAddr,
    endpoint: Option<Endpoint>,
    handler: Arc<dyn Http3EventHandler>,
    running: Arc<AtomicBool>,
}

impl EnterpriseHttp3 for EnterpriseHttp3Server {
    fn new(addr: &str, handler: Arc<dyn Http3EventHandler>) -> Result<Self> {
        let addr = addr
            .to_socket_addrs()
            .map_err(|e| tquic::Error::InvalidConfig(e.to_string()))?
            .next()
            .ok_or(tquic::Error::InvalidConfig("no address found".into()))?;

        Ok(Self {
            addr,
            endpoint: None,
            handler,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    fn start(&mut self) -> Result<()> {
        let mut config = create_server_config()?;
        let mut endpoint = Endpoint::new(config, true)?;
        endpoint.listen(&self.addr)?;
        self.endpoint = Some(endpoint);
        self.running.store(true, Ordering::SeqCst);
        let endpoint = self.endpoint.as_mut().unwrap().clone();
        let handler = self.handler.clone();
        let running = self.running.clone();

        may::go!(move || {
            info!("EnterpriseHttp3Server started on {}", endpoint.local_addr().unwrap());
            while running.load(Ordering::SeqCst) {
                if let Some(mut conn) = endpoint.accept() {
                    info!(
                        "New connection from: {}",
                        conn.peer_addr().unwrap()
                    );
                    let handler_clone = handler.clone();
                    may::go!(move || {
                        if let Err(e) = handle_connection(conn, handler_clone) {
                            error!("Connection handling failed: {:?}", e);
                        }
                    });
                }
            }
        });
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn send_request(&self, _request: Http3Request) -> Result<()> {
        // Server does not send requests in this model
        Ok(())
    }
}

/// EnterpriseHttp3Client 结构体
pub struct EnterpriseHttp3Client {
    remote_addr: SocketAddr,
    local_addr: SocketAddr,
    endpoint: Endpoint,
    connection: Option<Connection>,
    handler: Arc<dyn Http3EventHandler>,
    running: Arc<AtomicBool>,
}

impl EnterpriseHttp3 for EnterpriseHttp3Client {
    fn new(addr: &str, handler: Arc<dyn Http3EventHandler>) -> Result<Self> {
        let remote_addr = addr
            .to_socket_addrs()
            .map_err(|e| tquic::Error::InvalidConfig(e.to_string()))?
            .next()
            .ok_or(tquic::Error::InvalidConfig("no address found".into()))?;
        
        let local_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        
        let mut config = create_client_config()?;
        let endpoint = Endpoint::new(config, false)?;

        Ok(Self {
            remote_addr,
            local_addr,
            endpoint,
            connection: None,
            handler,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    fn start(&mut self) -> Result<()> {
        let connecting = self.endpoint.connect(&self.remote_addr, "localhost")?;
        let conn = may::coroutine::scope(|s| {
            s.spawn(move || {
                // You can add timeout logic here if needed
                connecting.wait()
            }).join().unwrap()
        })?;
        
        info!("Connected to {}", conn.peer_addr()?);
        self.connection = Some(conn);
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(mut conn) = self.connection.take() {
            conn.close(0, b"done").ok();
        }
    }

    fn send_request(&self, request: Http3Request) -> Result<()> {
        if let Some(conn) = &mut self.connection {
            let (mut stream, _) = conn.h3_mut().open_stream()?;

            let headers: Vec<Header> = request
                .headers
                .into_iter()
                .map(|(n, v)| Header::new(n.as_bytes(), v.as_bytes()))
                .collect();
            
            stream.send_headers(&headers)?;
            
            if let Some(body) = request.body {
                stream.send_body(&body)?;
            }
            stream.shutdown()?;
        }
        Ok(())
    }
}


fn handle_connection(mut conn: Connection, handler: Arc<dyn Http3EventHandler>) -> Result<()> {
    while let Ok((mut stream, _)) = conn.h3_mut().accept() {
        let handler_clone = handler.clone();
        may::go!(move || {
            let stream_id = stream.id();
            info!("New stream {} accepted", stream_id);

            match stream.recv_headers() {
                Ok(headers) => {
                    let http3_req = parse_request_headers(headers);
                    match handler_clone.handle_request(http3_req) {
                        Ok(resp) => {
                            let mut resp_headers = vec![
                                Header::new(b":status", resp.status.to_string().as_bytes()),
                            ];
                            for (n, v) in resp.headers {
                                resp_headers.push(Header::new(n.as_bytes(), v.as_bytes()));
                            }

                            if let Err(e) = stream.send_headers(&resp_headers) {
                                error!("Failed to send response headers: {:?}", e);
                                return;
                            }
                            if let Some(body) = resp.body {
                                if let Err(e) = stream.send_body(&body) {
                                     error!("Failed to send response body: {:?}", e);
                                     return;
                                }
                            }
                            if let Err(e) = stream.shutdown() {
                                error!("Failed to shutdown stream: {:?}", e);
                            }
                        }
                        Err(e) => {
                            error!("Handler failed to process request: {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to receive headers: {:?}", e);
                }
            }
        });
    }
    Ok(())
}

fn parse_request_headers(headers: Vec<Header>) -> Http3Request {
    let mut req = Http3Request {
        method: String::new(),
        scheme: String::new(),
        authority: String::new(),
        path: String::new(),
        headers: Vec::new(),
        body: None,
    };

    for header in &headers {
        // FIX: Use `header.name()` and `header.value()` which are available via NameValue trait
        match header.name() {
            b":method" => req.method = String::from_utf8_lossy(header.value()).to_string(),
            b":scheme" => req.scheme = String::from_utf8_lossy(header.value()).to_string(),
            b":authority" => req.authority = String::from_utf8_lossy(header.value()).to_string(),
            b":path" => req.path = String::from_utf8_lossy(header.value()).to_string(),
            _ => req.headers.push((
                String::from_utf8_lossy(header.name()).to_string(),
                String::from_utf8_lossy(header.value()).to_string(),
            )),
        }
    }
    req
}


// CHANGED: Refactored config creation to use the builder pattern
fn create_server_config() -> Result<Config> {
    // Generate temporary self-signed certificate for the example
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.serialize_der().unwrap();
    let priv_key = cert.serialize_private_key_der();
    fs::write("temp_cert.der", &cert_der)?;
    fs::write("temp_key.der", &priv_key)?;

    let mut config = Config::builder();
    config
        .set_application_protos(&[b"h3"])?
        .load_cert_chain_from_der_file("temp_cert.der")?
        .load_priv_key_from_der_file("temp_key.der")?
        .set_max_idle_timeout(30_000)?
        .set_max_udp_payload_size(1350)?
        .set_initial_max_data(10_000_000)?
        .set_initial_max_stream_data_bidi_local(1_000_000)?
        .set_initial_max_stream_data_bidi_remote(1_000_000)?
        .set_initial_max_streams_bidi(100)?
        .set_initial_max_streams_uni(100)?
        .verify_peer(false); // For self-signed cert

    Ok(config.build()?)
}

// CHANGED: Refactored config creation to use the builder pattern
fn create_client_config() -> Result<Config> {
    let mut config = Config::builder();
    config
        .set_application_protos(&[b"h3"])?
        .verify_peer(false) // In a real app, you should verify the server cert
        .set_max_idle_timeout(30_000)?
        .set_max_udp_payload_size(1350)?
        .set_initial_max_data(10_000_000)?
        .set_initial_max_stream_data_bidi_local(1_000_000)?
        .set_initial_max_stream_data_bidi_remote(1_000_000)?
        .set_initial_max_streams_bidi(100)?
        .set_initial_max_streams_uni(100)?;

    Ok(config.build()?)
}