// src/main.rs

#![allow(warnings)]
use bytes::Bytes;
use may::go;
use std::sync::Arc;
use std::time::Duration;

mod http3;

// FIX: Import types that are now re-exported from the http3 module
use crate::http3::{
    EnterpriseHttp3, EnterpriseHttp3Server, Http3EventHandler, Http3Request, Http3Response,
    Http3Result,
};

slint::include_modules!();

// Implement the Http3EventHandler trait
struct AppHandler;

impl Http3EventHandler for AppHandler {
    // FIX: Corrected the function signature to match the trait definition
    fn handle_request(&self, request: Http3Request) -> Http3Result<Http3Response> {
        println!("Received request: {:?}", request);
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
            ("server".to_string(), "colacoder-http3".to_string()),
        ];
        let body = Some("Hello from ColaCoder HTTP/3 Server!".as_bytes().to_vec());
        Ok(Http3Response::new(200, headers, body))
    }

    fn handle_response(&self, response: Http3Response) {
        println!("Received response: {:?}", response);
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // Initialize logger
    env_logger::init();
    
    // Initialize the `may` runtime
    may::config().set_workers(4);

    let main_window = MainWindow::new()?;
    let handler = Arc::new(AppHandler);
    let mut http3_server = EnterpriseHttp3Server::new("127.0.0.1:8080", handler.clone()).unwrap();

    http3_server.start().expect("Failed to start server");

    let _http3_handle = may::go!(move || {
        // The server logic is now running inside the EnterpriseHttp3Server::start method
        // This thread can be used for other tasks or just sleep
        loop {
            may::coroutine::sleep(Duration::from_secs(1));
        }
    });

    main_window.on_request_increase_value({
        let main_window_handle = main_window.as_weak();
        move || {
            let main_window = main_window_handle.unwrap();
            main_window.set_counter(main_window.get_counter() + 1);
        }
    });

    main_window.run()
}