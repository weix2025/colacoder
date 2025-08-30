// src/main.rs

#![allow(warnings)]
use slint::*;
use std::sync::Arc;
use std::net::SocketAddr;
use may::{self, coroutine};
use log::*;

mod http3;
use http3::{EnterpriseHttp3Server, Http3EventHandler, Http3Request, Http3Response, EnterpriseHttp3, TquicError, generate_temp_certificate};

slint::include_modules!();

struct AppHandler {
    window: slint::Weak<MainWindow>,
}

impl Http3EventHandler for AppHandler {
    fn on_request(&self, request: Http3Request) -> Http3Response {
        let ui = self.window.upgrade().unwrap();
        
        // 更新请求计数
        let current_count = ui.get_http3_request_count();
        ui.set_http3_request_count(current_count + 1);
        
        // 更新消息
        let message = std::format!("HTTP/3 请求: {} {}", request.method, request.path);
        ui.set_message(message.into());
        
        // 创建响应
        Http3Response::new(200)
            .with_header("content-type".to_string(), "text/plain; charset=utf-8".to_string())
            .text("Hello from HTTP/3! 来自HTTP/3的问候!")
    }
    
    fn on_connection_established(&self) {
        let ui = self.window.upgrade().unwrap();
        ui.set_http3_connected(true);
        ui.set_http3_status("已连接".into());
        info!("HTTP/3 连接已建立");
    }
    
    fn on_connection_closed(&self) {
        let ui = self.window.upgrade().unwrap();
        ui.set_http3_connected(false);
        ui.set_http3_status("已断开".into());
        info!("HTTP/3 连接已关闭");
    }
    
    fn on_error(&self, error: String) {
        let ui = self.window.upgrade().unwrap();
        ui.set_http3_error(error.clone().into());
        ui.set_http3_status("错误".into());
        log::error!("HTTP/3 错误: {}", error);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    env_logger::init();
    may::config().set_stack_size(2 * 1024 * 1024);
    
    let ui = MainWindow::new()?;
    let ui_handle = ui.as_weak();
    
    // 生成临时证书
    if let Err(e) = generate_temp_certificate() {
        eprintln!("生成证书失败: {}", e);
        return Ok(());
    }
    
    // 启动HTTP/3服务器
    let handler = Arc::new(AppHandler {
        window: ui_handle.clone(),
    });
    
    unsafe {
        coroutine::spawn(move || {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        match EnterpriseHttp3Server::new(addr, handler) {
            Ok(mut server) => {
                info!("正在启动HTTP/3服务器，地址: {}", addr);
                if let Err(e) = server.start() {
                    log::error!("启动HTTP/3服务器失败: {}", e);
                }
            }
            Err(e) => {
                log::error!("创建HTTP/3服务器失败: {}", e);
            }
         }
         });
     }

    let on_button_clicked = {
        let ui_weak = ui.as_weak();
        move || {
            let ui = ui_weak.unwrap();
            let current_count = ui.get_request_count();
            ui.set_request_count(current_count + 1);
            ui.set_message("Button clicked!".into());
        }
    };
    ui.on_button_clicked(on_button_clicked);

    ui.run()
}