slint::include_modules!();

use std::sync::{Arc, Mutex};
use std::net::SocketAddr;
use std::io;
use may::coroutine;
use may::sync::mpsc;
use may::go;
use may_minihttp::{HttpServer, HttpService, Request, Response};
use log::{info, error};

mod http3;
use http3::{Http3Service, Http3Request, EnterpriseHttp3Server, EnterpriseHttp3Client};

// 定义HTTP服务状态结构体
pub struct ServerStatus {
    is_running: bool,
    request_count: usize,
    last_request: String,
    http3_request_count: usize,
    should_shutdown: bool,
    active_connections: usize,
    total_connections: usize,
}

impl Default for ServerStatus {
    fn default() -> Self {
        Self {
            is_running: false,
            request_count: 0,
            last_request: String::new(),
            http3_request_count: 0,
            should_shutdown: false,
            active_connections: 0,
            total_connections: 0,
        }
    }
}

// 实现HTTP服务
#[derive(Clone)]
struct SimpleHttpService {
    status: Arc<Mutex<ServerStatus>>,
    sender: mpsc::Sender<String>,
}

impl HttpService for SimpleHttpService {
    fn call(&mut self, req: Request, res: &mut Response) -> io::Result<()> {
        // 处理请求
        let path = req.path().to_string();
        
        // 更新状态
        let mut status = self.status.lock().unwrap();
        status.request_count += 1;
        status.last_request = path.clone();
        
        // 发送消息到UI线程
        let message = format!("收到HTTP/1.1请求: {} (总请求数: {})", path, status.request_count);
        let _ = self.sender.send(message);
        
        // 返回响应 - 使用静态字符串而不是动态创建的字符串
        res.body("Hello from Slint + May! (HTTP/1.1)");
        Ok(())
    }
}

// 实现HTTP/3服务
#[derive(Clone)]
struct SimpleHttp3Service {
    status: Arc<Mutex<ServerStatus>>,
    sender: mpsc::Sender<String>,
}

impl Http3Service for SimpleHttp3Service {
    fn handle_request(&self, request: Http3Request) -> http3::Http3Result<http3::Http3Response> {
        use http3::{Http3Response, Header};
        use bytes::Bytes;
        use std::time::Instant;
        
        // 处理请求
        let path = request.path.clone();
        let method = request.method.clone();
        
        // 更新状态
        let mut status = self.status.lock().unwrap();
        status.http3_request_count += 1;
        status.last_request = format!("{} {}", method, path);
        
        // 发送消息到UI线程
        let message = format!("收到HTTP/3请求: {} {} (总请求数: {})", method, path, status.http3_request_count);
        let _ = self.sender.send(message);
        
        // 构建响应内容
        let response_body = format!("Hello from Slint + TQUIC! (HTTP/3)\nMethod: {}\nPath: {}\nHeaders: {:?}", method, path, request.headers);
        
        // 返回HTTP/3响应
        Ok(Http3Response {
            status: 200,
            headers: vec![
                Header::new(b":status", b"200"),
                Header::new(b"content-type", b"text/plain"),
                Header::new(b"server", b"slint-tquic-server/1.0"),
            ],
            body: Bytes::from(response_body),
            content_type: "text/plain".to_string(),
            cache_control: None,
            timestamp: Instant::now(),
        })
    }
}

fn main() -> Result<(), slint::PlatformError> {
    env_logger::init();
    
    // 初始化may运行时
    may::config().set_workers(2);
    
    // 创建状态共享对象
    let server_status = Arc::new(Mutex::new(ServerStatus {
        is_running: false,
        request_count: 0,
        last_request: String::new(),
        http3_request_count: 0,
        should_shutdown: false,
        active_connections: 0,
        total_connections: 0,
    }));
    
    // 创建通信通道
    let (sender, receiver) = mpsc::channel();
    
    // 加载UI
    let main_window = MainWindow::new()?;
    
    // 克隆UI句柄用于回调
    let main_window_weak = main_window.as_weak();
    
    // 设置按钮点击回调
    main_window.on_button_clicked(move || {
        let ui = main_window_weak.unwrap();
        let input_text = ui.get_input_text().to_string();
        
        if !input_text.is_empty() {
            ui.set_message(format!("你输入了: {}", input_text).into());
        } else {
            ui.set_message("请在输入框中输入文本".into());
        }
    });
    
    // 设置HTTP/1.1请求按钮回调
    let main_window_weak_http1 = main_window.as_weak();
    main_window.on_request_http1(move || {
        if let Some(ui) = main_window_weak_http1.upgrade() {
            ui.set_message("正在发送HTTP/1.1请求...".into());
            
            // 这里可以添加实际的HTTP/1.1客户端请求逻辑
            // 目前只是显示消息
            ui.set_message("HTTP/1.1请求已发送，请查看服务器日志".into());
        }
    });
    
    // 设置HTTP/3请求按钮回调
    let main_window_weak_http3 = main_window.as_weak();
    let sender_for_http3 = sender.clone();
    main_window.on_request_http3(move || {
        if let Some(ui) = main_window_weak_http3.upgrade() {
            ui.set_message("正在发送HTTP/3请求...".into());
            
            // 启动HTTP/3客户端请求
            let sender_for_client = sender_for_http3.clone();
            // 使用may的go!宏来启动协程
            let _handle = go!(move || {
                // 在may协程中使用tokio运行时执行async函数
                let rt = tokio::runtime::Runtime::new().unwrap();
                match rt.block_on(send_http3_request(sender_for_client)) {
                    Ok(_) => {
                        info!("HTTP/3请求发送成功");
                    }
                    Err(e) => {
                        error!("HTTP/3请求发送失败: {}", e);
                    }
                }
            });
        }
    });
    
    // 启动HTTP/1.1服务器协程
    let server_status_clone = server_status.clone();
    let sender_clone = sender.clone();
    unsafe {
        coroutine::spawn(move || {
            // 更新服务器状态
            {
                let mut status = server_status_clone.lock().unwrap();
                status.is_running = true;
            }
            
            // 发送启动消息
            let _ = sender_clone.send("HTTP/1.1服务器已启动在 http://127.0.0.1:8000".to_string());
            
            // 创建HTTP服务
            let service = SimpleHttpService {
                status: server_status_clone.clone(),
                sender: sender_clone,
            };
            
            // 启动HTTP服务器
            let server = HttpServer(service).start("127.0.0.1:8000").unwrap();
            server.join().unwrap();
        });
    }
    
    // 启动企业级HTTP/3服务器 - 在主线程中创建以避免Send trait问题
    let http3_addr = "127.0.0.1:8443".parse().unwrap();
    let http3_service_arc = Arc::new(http3::DefaultEnterpriseHttp3Service::new());
    let http3_server_status = Arc::new(Mutex::new(ServerStatus::default()));
    let http3_server = http3::EnterpriseHttp3Server::new(
        http3_service_arc,
        http3_server_status.clone(),
    ).map_err(|e| slint::PlatformError::Other(format!("Failed to create HTTP/3 server: {:?}", e).into()))?;
    
    // 在主线程中启动企业级HTTP/3服务器
    let sender_for_http3 = sender.clone();
    let http3_server_clone = http3_server.clone();
    let _http3_handle = unsafe {
        may::coroutine::spawn(move || {
            may::coroutine::scope(|scope| {
                unsafe {
                    scope.spawn(|| {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        rt.block_on(async {
                            let mut server = http3_server_clone;
                            if let Err(e) = server.start(http3_addr, sender_for_http3.clone()).await {
                                error!("Enterprise HTTP/3 server start failed: {:?}", e);
                            }
                            if let Err(e) = server.run_event_loop().await {
                                error!("Enterprise HTTP/3 server event loop failed: {:?}", e);
                            }
                        });
                    });
                }
            });
        })
    };
    
    info!("Enterprise HTTP/3服务器协程启动成功");
    
    // 设置初始服务器状态
    main_window.set_server_status("HTTP服务器准备启动...".into());
    main_window.set_request_count(0);
    main_window.set_http3_status("HTTP/3服务器准备启动...".into());
    main_window.set_http3_error("".into());
    main_window.set_http3_connected(false);
    
    // 创建UI更新协程
    let main_window_weak_clone = main_window.as_weak();
    let server_status_for_ui = server_status.clone();
    unsafe {
        coroutine::spawn(move || {
            // 首先更新服务器状态为启动中
            {
                let ui_handle = main_window_weak_clone.clone();
                slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_handle.upgrade() {
                        ui.set_server_status("HTTP服务器启动中...".into());
                    }
                }).ok();
            }
            
            // 等待服务器启动
            std::thread::sleep(std::time::Duration::from_millis(500));
            
            // 监听消息通道
            while let Ok(message) = receiver.recv() {
                // 获取最新的请求计数
                let request_count = {
                    let status = server_status_for_ui.lock().unwrap();
                    status.request_count
                };
                
                // 获取HTTP/3请求计数
                let http3_request_count = {
                    let status = server_status_for_ui.lock().unwrap();
                    status.http3_request_count
                };
                
                // 使用invoke_from_event_loop安全地更新UI
                let ui_handle = main_window_weak_clone.clone();
                let message_str = message.to_string();
                slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui_handle.upgrade() {
                        ui.set_message(message_str.clone().into());
                        ui.set_request_count(request_count as i32);
                        ui.set_http3_request_count(http3_request_count as i32);
                        
                        // 更新HTTP/3状态
                        if message_str.contains("HTTP/3服务器已启动") {
                            ui.set_http3_status("HTTP/3服务器运行中".into());
                            ui.set_http3_connected(true);
                            ui.set_http3_error("".into());
                        } else if message_str.contains("HTTP/3服务器启动失败") {
                            ui.set_http3_status("HTTP/3服务器启动失败".into());
                            ui.set_http3_connected(false);
                            ui.set_http3_error(message_str.clone().into());
                        } else if message_str.contains("HTTP/3") && message_str.contains("错误") {
                            ui.set_http3_status("HTTP/3服务器错误".into());
                            ui.set_http3_connected(false);
                            ui.set_http3_error(message_str.clone().into());
                        } else if message_str.contains("HTTP/3请求") {
                            ui.set_http3_status("HTTP/3服务器活跃".into());
                            ui.set_http3_connected(true);
                        }
                        
                        // 更新服务器状态
                        let total_requests = request_count + http3_request_count;
                        if total_requests == 1 {
                            ui.set_server_status("HTTP服务器运行中 - 已接收第一个请求".into());
                        } else if total_requests > 1 {
                            ui.set_server_status(format!("HTTP服务器运行中 - 活跃 (HTTP/1.1: {}, HTTP/3: {})", request_count, http3_request_count).into());
                        }
                    }
                }).ok();
            }
        });
    }
    
    // HTTP/3服务器事件处理将在UI事件循环中进行
    
    // 使用定时器在UI事件循环中处理企业级HTTP/3服务器事件
    let http3_server_for_timer = http3_server.clone();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
        if let Err(e) = http3_server_for_timer.process_events() {
            error!("Enterprise HTTP/3服务器事件处理失败: {}", e);
        }
    });
    
    // 显示窗口
     main_window.show()?;
    
    // 运行事件循环
    slint::run_event_loop()
}

// HTTP/3客户端请求函数
async fn send_http3_request(sender: mpsc::Sender<String>) -> Result<(), Box<dyn std::error::Error>> {
    info!("开始发送HTTP/3请求");
    
    // 设置服务器地址
    let server_addr: SocketAddr = "127.0.0.1:8443".parse()?;
    
    // 使用优化的HTTP/3请求发送函数
    match http3::send_enterprise_http3_request(server_addr, "/", "GET", None, None).await {
        Ok(_) => {
            let _ = sender.send("HTTP/3请求处理完成".to_string());
            info!("HTTP/3请求发送成功");
        }
        Err(e) => {
            let error_msg = format!("HTTP/3请求失败: {}", e);
            let _ = sender.send(error_msg);
            error!("HTTP/3请求失败: {}", e);
        }
    }
    
    Ok(())
}
