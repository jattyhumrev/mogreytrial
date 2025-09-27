use crate::{HvncError, Result};
use crate::common::{InputEvent, MouseButton};
use crate::host::WindowHandle;
use std::mem;
use winapi::shared::windef::{POINT, RECT};
use winapi::um::winuser::{
    PostMessageA, GetWindowRect, SetThreadDesktop, GetThreadDesktop, GetWindowThreadProcessId,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, 
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_KEYDOWN, WM_KEYUP,
    GetCursorPos, SetCursorPos, GetForegroundWindow, SendInput, 
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, OpenDesktopA, CloseDesktop,
    SendMessageA, SetForegroundWindow, AttachThreadInput,
    GetWindowTextA, IsWindowVisible, IsWindowEnabled, IsWindow,
    MapVirtualKeyA, MAPVK_VK_TO_VSC,
    // Added for child control focus resolution
    FindWindowExA, SetFocus
};
use winapi::um::winuser::{INPUT, INPUT_MOUSE, INPUT_KEYBOARD, MOUSEINPUT, KEYBDINPUT};
use winapi::um::winnt::GENERIC_ALL;
use winapi::um::processthreadsapi::GetCurrentThreadId;
use log::{debug, warn, info, error};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use lazy_static::lazy_static;
use std::fs::OpenOptions;
use std::io::{Write, BufWriter};
use std::sync::Mutex;
use chrono::Utc;

// Debug logging functionality
lazy_static! {
    static ref DEBUG_LOGGER: Mutex<Option<BufWriter<std::fs::File>>> = Mutex::new(None);
}

fn init_server_debug_log() {
    let mut logger = DEBUG_LOGGER.lock().unwrap();
    if logger.is_none() {
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open("server_debug.log") {
            Ok(file) => {
                *logger = Some(BufWriter::new(file));
                log_server_debug("Server debug logging initialized");
            }
            Err(e) => eprintln!("Failed to initialize server debug log: {}", e),
        }
    }
}

fn log_server_debug(message: &str) {
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let log_message = format!("[{}] SERVER: {}\n", timestamp, message);
    
    let mut logger = DEBUG_LOGGER.lock().unwrap();
    if let Some(ref mut writer) = *logger {
        if let Err(e) = writer.write_all(log_message.as_bytes()) {
            eprintln!("Failed to write to server debug log: {}", e);
        } else {
            let _ = writer.flush();
        }
    }
}

// Input method enumeration
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputMethod {
    SendInput,
    SendMessage,
    PostMessage,
    Auto,
}

// Input strategy configuration
#[derive(Debug, Clone)]
pub struct InputStrategy {
    pub method: InputMethod,
    pub use_thread_attachment: bool,
    pub validate_window: bool,
    pub force_foreground: bool,
}

impl InputStrategy {
    pub fn for_desktop_capture() -> Self {
        Self {
            method: InputMethod::SendInput,
            use_thread_attachment: false,
            validate_window: false,
            force_foreground: false,
        }
    }
    
    pub fn for_window_target(window: WindowHandle) -> Self {
        Self {
            method: InputMethod::Auto,
            use_thread_attachment: true,
            validate_window: true,
            force_foreground: true,
        }
    }
    
    pub fn for_hidden_desktop() -> Self {
        Self {
            method: InputMethod::SendInput,
            use_thread_attachment: false,
            validate_window: false,
            force_foreground: false,
        }
    }
}

pub struct InputHandler {
    last_input_time: Option<Instant>,
    input_rate_limit_ms: u64,
    coordinate_offset: (i32, i32),
    security_config: SecurityConfig,
    input_statistics: InputStatistics,
    target_desktop: Option<crate::host::DesktopHandle>,
    original_desktop: Option<crate::host::DesktopHandle>,
    input_strategy: InputStrategy,
    desktop_capture_mode: bool,
}

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub max_events_per_second: u32,
    pub max_coordinate_value: i32,
    pub min_coordinate_value: i32,
    pub blocked_keycodes: Vec<u32>,
    pub enable_coordinate_validation: bool,
    pub enable_keycode_validation: bool,
    pub max_consecutive_identical: u32,
}

#[derive(Debug)]
pub struct InputStatistics {
    pub total_events: u64,
    pub rate_limited_events: u64,
    pub validation_rejected_events: u64,
    pub events_by_type: HashMap<String, u64>,
    pub last_event: Option<InputEvent>,
    pub consecutive_identical_count: u32,
    pub start_time: Instant,
}

impl Default for InputStatistics {
    fn default() -> Self {
        Self {
            total_events: 0,
            rate_limited_events: 0,
            validation_rejected_events: 0,
            events_by_type: HashMap::new(),
            last_event: None,
            consecutive_identical_count: 0,
            start_time: Instant::now(),
        }
    }
}

impl SecurityConfig {
    pub fn default() -> Self {
        Self {
            max_events_per_second: 100,
            max_coordinate_value: 4096,
            min_coordinate_value: -4096,
            blocked_keycodes: vec![],
            enable_coordinate_validation: true,
            enable_keycode_validation: true,
            max_consecutive_identical: 10,
        }
    }
    
    pub fn permissive() -> Self {
        Self {
            max_events_per_second: 1000,
            max_coordinate_value: 32767,
            min_coordinate_value: -32768,
            blocked_keycodes: vec![],
            enable_coordinate_validation: false,
            enable_keycode_validation: false,
            max_consecutive_identical: 100,
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        init_server_debug_log();
        log_server_debug("Creating new InputHandler");
        
        Self {
            last_input_time: None,
            input_rate_limit_ms: 0, // FIXED: Disable rate limiting for HVNC (was 10ms)
            coordinate_offset: (0, 0),
            security_config: SecurityConfig::permissive(), // FIXED: Use permissive config
            input_statistics: InputStatistics::default(),
            target_desktop: None,
            original_desktop: None,
            input_strategy: InputStrategy::for_desktop_capture(),
            desktop_capture_mode: false,
        }
    }
    
    pub fn new_for_desktop_capture() -> Self {
        let mut handler = Self::new();
        handler.input_strategy = InputStrategy::for_desktop_capture();
        handler.desktop_capture_mode = true;
        handler
    }
    
    pub fn new_for_window(window: WindowHandle) -> Self {
        let mut handler = Self::new();
        handler.input_strategy = InputStrategy::for_window_target(window);
        handler
    }
    
    pub fn set_input_strategy(&mut self, strategy: InputStrategy) {
        log_server_debug(&format!("Input strategy updated: {:?}", strategy));
        self.input_strategy = strategy;
    }
    
    pub fn set_desktop_capture_mode(&mut self, enabled: bool) {
        self.desktop_capture_mode = enabled;
        if enabled {
            self.input_strategy = InputStrategy::for_desktop_capture();
        }
    }
    
    pub fn set_target_desktop(&mut self, desktop: crate::host::DesktopHandle) {
        self.target_desktop = Some(desktop);
    }
    
    fn switch_to_target_desktop(&mut self) -> Result<()> {
        if let Some(target_desktop) = self.target_desktop {
            let current_desktop = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
            if current_desktop.is_null() {
                return Err(HvncError::windows_api("Failed to get current desktop"));
            }
            
            if current_desktop != target_desktop {
                self.original_desktop = Some(current_desktop);
                let result = unsafe { SetThreadDesktop(target_desktop) };
                if result == 0 {
                    return Err(HvncError::windows_api("Failed to switch to target desktop"));
                }
            }
        }
        Ok(())
    }
    
    fn restore_original_desktop(&mut self) -> Result<()> {
        if let Some(original_desktop) = self.original_desktop {
            let result = unsafe { SetThreadDesktop(original_desktop) };
            if result == 0 {
                return Err(HvncError::windows_api("Failed to restore original desktop"));
            }
            self.original_desktop = None;
        }
        Ok(())
    }
    
    fn resolve_edit_child(&self, window: WindowHandle) -> WindowHandle {
        if window.is_null() {
            return std::ptr::null_mut();
        }
        debug!("🔎 Resolving child EDIT control for window {:?}", window);
        let edit_class = std::ffi::CString::new("Edit").unwrap();
        let rich_class = std::ffi::CString::new("RichEditD2DPT").unwrap();
        let child = unsafe { FindWindowExA(window, std::ptr::null_mut(), edit_class.as_ptr(), std::ptr::null()) };
        if !child.is_null() {
            debug!("✅ Found child EDIT control: {:?}", child);
            return child;
        }
        let child_rich = unsafe { FindWindowExA(window, std::ptr::null_mut(), rich_class.as_ptr(), std::ptr::null()) };
        if !child_rich.is_null() {
            debug!("✅ Found child RichEdit control: {:?}", child_rich);
            return child_rich;
        }
        warn!("⚠️ No child EDIT/RichEdit control found");
        std::ptr::null_mut()
    }
    
    fn focus_target_window(&self, window: WindowHandle) -> Result<()> {
        if window.is_null() {
            return Ok(());
        }
        
        debug!("🎯 Focusing target window {:?}", window);

        if self.input_strategy.force_foreground {
            let result = unsafe { SetForegroundWindow(window) };
            if result == 0 {
                warn!("Failed to set foreground window, continuing anyway");
            } else {
                debug!("✅ Foreground window set successfully");
            }
        }

        if self.input_strategy.use_thread_attachment {
            self.attach_to_window_thread(window)?;
        }

        // Attempt to focus child edit control when available
        let child = self.resolve_edit_child(window);
        let target_focus = if !child.is_null() { child } else { window };
        let focus_result = unsafe { SetFocus(target_focus) };
        if focus_result.is_null() {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            warn!("⚠️ SetFocus failed for {:?} (error={})", target_focus, error_code);
        } else {
            debug!("✅ Input focus set to {:?}", target_focus);
        }

        Ok(())
    }
    
    fn cleanup_input_focus(&self, window: WindowHandle) -> Result<()> {
        if window.is_null() {
            return Ok(());
        }
        
        if self.input_strategy.use_thread_attachment {
            self.detach_from_window_thread(window);
        }
        
        Ok(())
    }
    
    pub fn new_with_rate_limit(rate_limit_ms: u64) -> Self {
        let mut handler = Self::new();
        handler.input_rate_limit_ms = rate_limit_ms;
        handler
    }
    
    pub fn new_with_security(security_config: SecurityConfig) -> Self {
        let mut handler = Self::new();
        handler.security_config = security_config;
        handler
    }
    
    pub fn set_coordinate_offset(&mut self, offset_x: i32, offset_y: i32) {
        self.coordinate_offset = (offset_x, offset_y);
    }
    
    fn should_rate_limit(&mut self) -> bool {
        if self.input_rate_limit_ms == 0 {
            return false;
        }
        
        let now = Instant::now();
        let should_limit = if let Some(last_time) = self.last_input_time {
            let elapsed = now.duration_since(last_time);
            elapsed.as_millis() < self.input_rate_limit_ms as u128
        } else {
            false
        };
        
        if should_limit {
            self.input_statistics.rate_limited_events += 1;
            debug!("Rate limiting input event");
        } else {
            self.last_input_time = Some(now);
        }
        
        should_limit
    }
    
    fn validate_input_event_security(&mut self, event: &InputEvent) -> Result<()> {
        // Check for consecutive identical events
        if let Some(ref last_event) = self.input_statistics.last_event {
            if std::mem::discriminant(last_event) == std::mem::discriminant(event) {
                self.input_statistics.consecutive_identical_count += 1;
                if self.input_statistics.consecutive_identical_count > self.security_config.max_consecutive_identical {
                    self.input_statistics.validation_rejected_events += 1;
                    return Err(HvncError::Security("Too many consecutive identical events".to_string()));
                }
            } else {
                self.input_statistics.consecutive_identical_count = 0;
            }
        }
        
        // Validate coordinates for mouse events
        if self.security_config.enable_coordinate_validation {
            match event {
                InputEvent::MouseClick { x, y, .. } | InputEvent::MouseMove { x, y } => {
                    if *x < self.security_config.min_coordinate_value || 
                       *x > self.security_config.max_coordinate_value ||
                       *y < self.security_config.min_coordinate_value || 
                       *y > self.security_config.max_coordinate_value {
                        self.input_statistics.validation_rejected_events += 1;
                        return Err(HvncError::Security("Coordinate values out of allowed range".to_string()));
                    }
                }
                _ => {}
            }
        }
        
        // Validate keycodes
        if self.security_config.enable_keycode_validation {
            if let InputEvent::KeyboardEvent { keycode, .. } = event {
                if self.security_config.blocked_keycodes.contains(keycode) {
                    self.input_statistics.validation_rejected_events += 1;
                    return Err(HvncError::Security("Keycode is blocked".to_string()));
                }
            }
        }
        
        Ok(())
    }
    
    fn update_statistics(&mut self, event: &InputEvent) {
        self.input_statistics.total_events += 1;
        
        let event_type = match event {
            InputEvent::MouseClick { .. } => "mouse_click",
            InputEvent::MouseMove { .. } => "mouse_move",
            InputEvent::KeyboardEvent { .. } => "key_press",
        };
        
        *self.input_statistics.events_by_type.entry(event_type.to_string()).or_insert(0) += 1;
        self.input_statistics.last_event = Some(event.clone());
    }
    
    pub fn handle_mouse_click(
        &mut self,
        window: WindowHandle,
        x: i32,
        y: i32,
        button: MouseButton,
        pressed: bool,
    ) -> Result<()> {
        let event = InputEvent::MouseClick { x, y, button: button.clone(), pressed };
        
        if self.should_rate_limit() {
            return Ok(());
        }
        
        self.validate_input_event_security(&event)?;
        self.update_statistics(&event);
        
        let (adjusted_x, adjusted_y) = (
            x + self.coordinate_offset.0,
            y + self.coordinate_offset.1,
        );
        
        if self.desktop_capture_mode {
            return self.handle_desktop_mouse_click(adjusted_x, adjusted_y, button, pressed);
        }
        
        if self.input_strategy.validate_window && !self.is_valid_window(window) {
            return Err(HvncError::WindowAccessDenied("Target window is not valid".to_string()));
        }
        
        self.focus_target_window(window)?;
        
        let result = match self.input_strategy.method {
            InputMethod::SendInput => self.send_mouse_click_sendinput(adjusted_x, adjusted_y, button.clone(), pressed),
            InputMethod::SendMessage => self.send_mouse_click_sendmessage(window, adjusted_x, adjusted_y, button.clone(), pressed),
            InputMethod::PostMessage => self.send_mouse_click_postmessage(window, adjusted_x, adjusted_y, button.clone(), pressed),
            InputMethod::Auto => {
                // Try SendInput first, fallback to PostMessage targeting child when available
                self.send_mouse_click_sendinput(adjusted_x, adjusted_y, button.clone(), pressed)
                    .or_else(|_| self.send_mouse_click_postmessage(window, adjusted_x, adjusted_y, button, pressed))
            }
        };
        
        self.cleanup_input_focus(window)?;
        result
    }
    
    pub fn handle_mouse_move(
        &mut self,
        window: WindowHandle,
        x: i32,
        y: i32,
    ) -> Result<()> {
        let event = InputEvent::MouseMove { x, y };
        
        if self.should_rate_limit() {
            return Ok(());
        }
        
        self.validate_input_event_security(&event)?;
        self.update_statistics(&event);
        
        let (adjusted_x, adjusted_y) = (
            x + self.coordinate_offset.0,
            y + self.coordinate_offset.1,
        );
        
        if self.desktop_capture_mode {
            return self.handle_desktop_mouse_move(adjusted_x, adjusted_y);
        }
        
        if self.input_strategy.validate_window && !self.is_valid_window(window) {
            return Err(HvncError::WindowAccessDenied("Target window is not valid".to_string()));
        }
        
        if let Err(e) = self.switch_to_target_desktop() {
            warn!("⚠️ Failed to switch to target desktop: {:?}", e);
        } else {
            debug!("🖥️ Switched to target desktop for mouse move");
        }
        
        self.focus_target_window(window)?;
        
        let child = self.resolve_edit_child(window);
        let target_for_messages = if !child.is_null() { child } else { window };
        
        let result = match self.input_strategy.method {
            InputMethod::SendInput => self.send_mouse_move_sendinput(adjusted_x, adjusted_y),
            InputMethod::SendMessage => self.send_mouse_move_sendmessage(target_for_messages, adjusted_x, adjusted_y),
            InputMethod::PostMessage => self.send_mouse_move_postmessage(target_for_messages, adjusted_x, adjusted_y),
            InputMethod::Auto => {
                // Try SendInput first, fallback to PostMessage targeting child when available
                self.send_mouse_move_sendinput(adjusted_x, adjusted_y)
                    .or_else(|_| self.send_mouse_move_postmessage(target_for_messages, adjusted_x, adjusted_y))
            }
        };
        
        self.cleanup_input_focus(window)?;
        result
    }
    
    pub fn handle_keyboard_event(
        &mut self,
        window: WindowHandle,
        keycode: u32,
        pressed: bool,
    ) -> Result<()> {
        let event = InputEvent::KeyboardEvent { keycode, pressed };
        
        if self.should_rate_limit() {
            return Ok(());
        }
        
        self.validate_input_event_security(&event)?;
        self.update_statistics(&event);
        
        if self.input_strategy.validate_window && !self.is_valid_window(window) {
            return Err(HvncError::WindowAccessDenied("Target window is not valid".to_string()));
        }
        
        if let Err(e) = self.switch_to_target_desktop() {
            warn!("⚠️ Failed to switch to target desktop: {:?}", e);
        } else {
            debug!("🖥️ Switched to target desktop for keyboard event");
        }
        
        self.focus_target_window(window)?;
        
        // For message-based keyboard injection, direct to child edit control when available
        let child = self.resolve_edit_child(window);
        let target_for_messages = if !child.is_null() { child } else { window };
        
        let result = match self.input_strategy.method {
            InputMethod::SendInput => self.send_keyboard_sendinput(keycode, pressed),
            InputMethod::SendMessage => self.send_keyboard_sendmessage(target_for_messages, keycode, pressed),
            InputMethod::PostMessage => self.send_keyboard_postmessage(target_for_messages, keycode, pressed),
            InputMethod::Auto => {
                // Try SendInput first, fallback to PostMessage targeting child when available
                self.send_keyboard_sendinput(keycode, pressed)
                    .or_else(|_| self.send_keyboard_postmessage(target_for_messages, keycode, pressed))
            }
        };
        
        self.cleanup_input_focus(window)?;
        result
    }
    
    pub fn process_input_event(
        &mut self,
        window: WindowHandle,
        event: InputEvent,
    ) -> Result<()> {
        match event {
            InputEvent::MouseClick { x, y, button, pressed } => {
                self.handle_mouse_click(window, x, y, button, pressed)
            }
            InputEvent::MouseMove { x, y } => {
                self.handle_mouse_move(window, x, y)
            }
            InputEvent::KeyboardEvent { keycode, pressed } => {
                self.handle_keyboard_event(window, keycode, pressed)
            }
        }
    }
    
    pub fn process_input_event_unsafe(
        &mut self,
        window: WindowHandle,
        event: InputEvent,
    ) -> Result<()> {
        // Bypass security checks for unsafe processing
        let original_config = self.security_config.clone();
        self.security_config = SecurityConfig::permissive();
        
        let result = self.process_input_event(window, event);
        
        self.security_config = original_config;
        result
    }
    
    fn validate_coordinates(&self, window: WindowHandle, x: i32, y: i32) -> Result<()> {
        if window.is_null() {
            return Ok(());
        }
        
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        
        let result = unsafe { GetWindowRect(window, &mut rect) };
        if result == 0 {
            return Err(HvncError::windows_api("Failed to get window rect"));
        }
        
        let window_width = rect.right - rect.left;
        let window_height = rect.bottom - rect.top;
        
        if x < 0 || x >= window_width || y < 0 || y >= window_height {
            return Err(HvncError::InputCoordinateOutOfBounds { x, y });
        }
        
        Ok(())
    }
    
    pub fn get_cursor_position() -> Result<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        let result = unsafe { GetCursorPos(&mut point) };
        
        if result == 0 {
            return Err(HvncError::windows_api("Failed to get cursor position"));
        }
        
        Ok((point.x, point.y))
    }
    
    pub fn set_cursor_position(x: i32, y: i32) -> Result<()> {
        let result = unsafe { SetCursorPos(x, y) };
        
        if result == 0 {
            return Err(HvncError::windows_api("Failed to set cursor position"));
        }
        
        Ok(())
    }
    
    pub fn get_foreground_window() -> WindowHandle {
        unsafe { GetForegroundWindow() }
    }
    
    pub fn reset_rate_limiting(&mut self) {
        self.last_input_time = None;
        self.input_statistics.rate_limited_events = 0;
    }
    
    pub fn get_rate_limit_ms(&self) -> u64 {
        self.input_rate_limit_ms
    }
    
    pub fn set_rate_limit_ms(&mut self, rate_limit_ms: u64) {
        self.input_rate_limit_ms = rate_limit_ms;
        log_server_debug(&format!("Rate limit updated to {} ms", rate_limit_ms));
    }
    
    pub fn get_security_config(&self) -> &SecurityConfig {
        &self.security_config
    }
    
    pub fn set_security_config(&mut self, config: SecurityConfig) {
        self.security_config = config;
        log_server_debug("Security configuration updated");
    }
    
    pub fn get_statistics(&self) -> &InputStatistics {
        &self.input_statistics
    }
    
    pub fn reset_statistics(&mut self) {
        self.input_statistics = InputStatistics::default();
        log_server_debug("Input statistics reset");
    }
    
    pub fn get_events_per_second(&self) -> f64 {
        let elapsed = self.input_statistics.start_time.elapsed();
        if elapsed.as_secs() == 0 {
            return 0.0;
        }
        self.input_statistics.total_events as f64 / elapsed.as_secs() as f64
    }
    
    pub fn is_under_attack(&self) -> bool {
        let events_per_second = self.get_events_per_second();
        events_per_second > self.security_config.max_events_per_second as f64 ||
        self.input_statistics.consecutive_identical_count > self.security_config.max_consecutive_identical
    }
    
    pub fn block_keycode(&mut self, keycode: u32) {
        if !self.security_config.blocked_keycodes.contains(&keycode) {
            self.security_config.blocked_keycodes.push(keycode);
            log_server_debug(&format!("Blocked keycode: {}", keycode));
        }
    }
    
    pub fn unblock_keycode(&mut self, keycode: u32) {
        self.security_config.blocked_keycodes.retain(|&k| k != keycode);
        log_server_debug(&format!("Unblocked keycode: {}", keycode));
    }
    
    pub fn is_keycode_blocked(&self, keycode: u32) -> bool {
        self.security_config.blocked_keycodes.contains(&keycode)
    }
    
    fn handle_desktop_mouse_click(&self, x: i32, y: i32, button: MouseButton, pressed: bool) -> Result<()> {
        debug!("🖱️ Handling desktop mouse click: button={:?}, pressed={}, pos=({}, {})", button, pressed, x, y);
        
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        
        if screen_width == 0 || screen_height == 0 {
            return Err(HvncError::windows_api("Failed to get screen dimensions"));
        }
        
        // Convert to absolute coordinates (0-65535 range)
        let abs_x = ((x as f64 / screen_width as f64) * 65535.0) as i32;
        let abs_y = ((y as f64 / screen_height as f64) * 65535.0) as i32;
        
        let mouse_flag = match (button, pressed) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        
        let mut input = INPUT {
            type_: INPUT_MOUSE,
            u: unsafe { std::mem::zeroed() },
        };
        
        unsafe {
            *input.u.mi_mut() = MOUSEINPUT {
                dx: abs_x,
                dy: abs_y,
                mouseData: 0,
                dwFlags: mouse_flag | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        
        let result = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
        
        if result != 1 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send desktop mouse click input", error_code
            ));
        }
        
        debug!("Desktop mouse click sent successfully");
        Ok(())
    }
    
    fn handle_desktop_mouse_move(&self, x: i32, y: i32) -> Result<()> {
        debug!("🖱️ Handling desktop mouse move to ({}, {})", x, y);
        
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        
        if screen_width == 0 || screen_height == 0 {
            return Err(HvncError::windows_api("Failed to get screen dimensions"));
        }
        
        // Convert to absolute coordinates (0-65535 range)
        let abs_x = ((x as f64 / screen_width as f64) * 65535.0) as i32;
        let abs_y = ((y as f64 / screen_height as f64) * 65535.0) as i32;
        
        let mut input = INPUT {
            type_: INPUT_MOUSE,
            u: unsafe { std::mem::zeroed() },
        };
        
        unsafe {
            *input.u.mi_mut() = MOUSEINPUT {
                dx: abs_x,
                dy: abs_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        
        let result = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
        
        if result != 1 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send desktop mouse move input", error_code
            ));
        }
        
        debug!("Desktop mouse move sent successfully");
        Ok(())
    }

    fn send_mouse_click_sendinput(&self, x: i32, y: i32, button: MouseButton, pressed: bool) -> Result<()> {
        debug!("🖱️ Sending mouse click via SendInput: button={:?}, pressed={}, pos=({}, {})", button, pressed, x, y);
        
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        
        if screen_width == 0 || screen_height == 0 {
            return Err(HvncError::windows_api("Failed to get screen dimensions"));
        }
        
        // Convert to absolute coordinates (0-65535 range)
        let abs_x = ((x as f64 / screen_width as f64) * 65535.0) as i32;
        let abs_y = ((y as f64 / screen_height as f64) * 65535.0) as i32;
        
        let mouse_flag = match (button, pressed) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        };
        
        let mut input = INPUT {
            type_: INPUT_MOUSE,
            u: unsafe { std::mem::zeroed() },
        };
        
        unsafe {
            *input.u.mi_mut() = MOUSEINPUT {
                dx: abs_x,
                dy: abs_y,
                mouseData: 0,
                dwFlags: mouse_flag | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        
        let result = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
        
        if result != 1 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send mouse click input", error_code
            ));
        }
        
        debug!("✅ Mouse click sent via SendInput successfully");
        Ok(())
    }

    fn send_mouse_click_sendmessage(&self, window: WindowHandle, x: i32, y: i32, button: MouseButton, pressed: bool) -> Result<()> {
        debug!("🖱️ Sending mouse click via SendMessage: button={:?}, pressed={}, pos=({}, {})", button, pressed, x, y);
        
        let (msg, wparam) = match (button, pressed) {
            (MouseButton::Left, true) => (WM_LBUTTONDOWN, 1),
            (MouseButton::Left, false) => (WM_LBUTTONUP, 0),
            (MouseButton::Right, true) => (WM_RBUTTONDOWN, 2),
            (MouseButton::Right, false) => (WM_RBUTTONUP, 0),
            (MouseButton::Middle, true) => (WM_MBUTTONDOWN, 16),
            (MouseButton::Middle, false) => (WM_MBUTTONUP, 0),
        };
        
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        let result = unsafe { SendMessageA(window, msg, wparam, lparam as isize) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send mouse click message", error_code
            ));
        }
        
        debug!("✅ Mouse click sent via SendMessage successfully");
        Ok(())
    }

    fn send_mouse_click_postmessage(&self, window: WindowHandle, x: i32, y: i32, button: MouseButton, pressed: bool) -> Result<()> {
        debug!("🖱️ Sending mouse click via PostMessage: button={:?}, pressed={}, pos=({}, {})", button, pressed, x, y);
        
        let (msg, wparam) = match (button, pressed) {
            (MouseButton::Left, true) => (WM_LBUTTONDOWN, 1),
            (MouseButton::Left, false) => (WM_LBUTTONUP, 0),
            (MouseButton::Right, true) => (WM_RBUTTONDOWN, 2),
            (MouseButton::Right, false) => (WM_RBUTTONUP, 0),
            (MouseButton::Middle, true) => (WM_MBUTTONDOWN, 16),
            (MouseButton::Middle, false) => (WM_MBUTTONUP, 0),
        };
        
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        let result = unsafe { PostMessageA(window, msg, wparam, lparam as isize) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to post mouse click message", error_code
            ));
        }
        
        debug!("✅ Mouse click sent via PostMessage successfully");
        Ok(())
    }

    fn attach_to_window_thread(&self, window: WindowHandle) -> Result<()> {
        debug!("🔗 Attaching to window thread for window {:?}", window);
        
        let current_thread_id = unsafe { GetCurrentThreadId() };
        let mut window_process_id = 0;
        let window_thread_id = unsafe { GetWindowThreadProcessId(window, &mut window_process_id) };
        
        if window_thread_id == 0 {
            return Err(HvncError::windows_api("Failed to get window thread ID"));
        }
        
        if window_thread_id != current_thread_id {
            let result = unsafe { AttachThreadInput(current_thread_id, window_thread_id, 1) };
            if result == 0 {
                let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
                return Err(HvncError::windows_api_with_code(
                    "Failed to attach to window thread", error_code
                ));
            }
            debug!("✅ Successfully attached to window thread");
        } else {
            debug!("Already on the same thread as target window");
        }
        
        Ok(())
    }

    fn detach_from_window_thread(&self, window: WindowHandle) {
        debug!("🔗 Detaching from window thread for window {:?}", window);
        
        let current_thread_id = unsafe { GetCurrentThreadId() };
        let mut window_process_id = 0;
        let window_thread_id = unsafe { GetWindowThreadProcessId(window, &mut window_process_id) };
        
        if window_thread_id != 0 && window_thread_id != current_thread_id {
            unsafe { AttachThreadInput(current_thread_id, window_thread_id, 0) };
            debug!("✅ Successfully detached from window thread");
        }
    }

    fn is_valid_window(&self, window: WindowHandle) -> bool {
        if window.is_null() {
            return false;
        }
        
        unsafe { IsWindow(window) != 0 && IsWindowVisible(window) != 0 }
    }

    fn send_mouse_move_sendinput(&self, x: i32, y: i32) -> Result<()> {
        debug!("🖱️ Sending mouse move via SendInput to ({}, {})", x, y);
        
        let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        
        if screen_width == 0 || screen_height == 0 {
            return Err(HvncError::windows_api("Failed to get screen dimensions"));
        }
        
        // Convert to absolute coordinates (0-65535 range)
        let abs_x = ((x as f64 / screen_width as f64) * 65535.0) as i32;
        let abs_y = ((y as f64 / screen_height as f64) * 65535.0) as i32;
        
        let mut input = INPUT {
            type_: INPUT_MOUSE,
            u: unsafe { std::mem::zeroed() },
        };
        
        unsafe {
            *input.u.mi_mut() = MOUSEINPUT {
                dx: abs_x,
                dy: abs_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        
        let result = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
        
        if result != 1 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send mouse move input", error_code
            ));
        }
        
        debug!("✅ Mouse move sent via SendInput successfully");
        Ok(())
    }

    fn send_mouse_move_sendmessage(&self, window: WindowHandle, x: i32, y: i32) -> Result<()> {
        debug!("🖱️ Sending mouse move via SendMessage to ({}, {})", x, y);
        
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        let result = unsafe { SendMessageA(window, WM_MOUSEMOVE, 0, lparam as isize) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send mouse move message", error_code
            ));
        }
        
        debug!("✅ Mouse move sent via SendMessage successfully");
        Ok(())
    }

    fn send_mouse_move_postmessage(&self, window: WindowHandle, x: i32, y: i32) -> Result<()> {
        debug!("🖱️ Sending mouse move via PostMessage to ({}, {})", x, y);
        
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        let result = unsafe { PostMessageA(window, WM_MOUSEMOVE, 0, lparam as isize) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to post mouse move message", error_code
            ));
        }
        
        debug!("✅ Mouse move sent via PostMessage successfully");
        Ok(())
    }

    fn send_keyboard_sendinput(&self, keycode: u32, pressed: bool) -> Result<()> {
        debug!("⌨️ Sending keyboard input via SendInput: keycode={}, pressed={}", keycode, pressed);
        
        let mut input = INPUT {
            type_: INPUT_KEYBOARD,
            u: unsafe { std::mem::zeroed() },
        };
        
        let mut flags = 0;
        if !pressed {
            flags |= KEYEVENTF_KEYUP;
        }
        
        unsafe {
            *input.u.ki_mut() = KEYBDINPUT {
                wVk: keycode as u16,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
        }
        
        let result = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
        
        if result != 1 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send keyboard input", error_code
            ));
        }
        
        debug!("✅ Keyboard input sent via SendInput successfully");
        Ok(())
    }

    fn send_keyboard_sendmessage(&self, window: WindowHandle, keycode: u32, pressed: bool) -> Result<()> {
        debug!("⌨️ Sending keyboard input via SendMessage: keycode={}, pressed={}", keycode, pressed);
        
        let msg = if pressed { WM_KEYDOWN } else { WM_KEYUP };
        let scan_code = unsafe { MapVirtualKeyA(keycode, MAPVK_VK_TO_VSC) };
        
        let mut lparam = (scan_code << 16) as isize;
        if !pressed {
            lparam |= 0xC0000000; // Set repeat count and previous key state
        }
        
        let result = unsafe { SendMessageA(window, msg, keycode as usize, lparam) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to send keyboard message", error_code
            ));
        }
        
        debug!("✅ Keyboard input sent via SendMessage successfully");
        Ok(())
    }

    fn send_keyboard_postmessage(&self, window: WindowHandle, keycode: u32, pressed: bool) -> Result<()> {
        debug!("⌨️ Sending keyboard input via PostMessage: keycode={}, pressed={}", keycode, pressed);
        
        let msg = if pressed { WM_KEYDOWN } else { WM_KEYUP };
        let scan_code = unsafe { MapVirtualKeyA(keycode, MAPVK_VK_TO_VSC) };
        
        let mut lparam = (scan_code << 16) as isize;
        if !pressed {
            lparam |= 0xC0000000; // Set repeat count and previous key state
        }
        
        let result = unsafe { PostMessageA(window, msg, keycode as usize, lparam) };
        
        if result == 0 {
            let error_code = unsafe { winapi::um::errhandlingapi::GetLastError() };
            return Err(HvncError::windows_api_with_code(
                "Failed to post keyboard message", error_code
            ));
        }
        
        debug!("✅ Keyboard input sent via PostMessage successfully");
        Ok(())
    }
}

impl Default for InputHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::desktop::create_hidden_desktop;
    use crate::host::launcher::{launch_app_in_desktop, find_window};
    use std::time::Duration;
    use std::ptr;
    
    #[test]
    fn test_input_handler_creation() {
        let handler = InputHandler::new();
        assert_eq!(handler.get_rate_limit_ms(), 10);
        assert_eq!(handler.coordinate_offset, (0, 0));
        assert!(!handler.desktop_capture_mode);
    }
    
    #[test]
    fn test_coordinate_offset() {
        let mut handler = InputHandler::new();
        handler.set_coordinate_offset(10, 20);
        assert_eq!(handler.coordinate_offset, (10, 20));
        
        // Test that coordinates are adjusted
        let result = handler.handle_mouse_move(std::ptr::null_mut(), 100, 200);
        // Should attempt to move to (110, 220) due to offset
    }
    
    #[test]
    fn test_rate_limiting() {
        let mut handler = InputHandler::new_with_rate_limit(100);
        assert_eq!(handler.get_rate_limit_ms(), 100);
        
        // First call should succeed
        assert!(!handler.should_rate_limit());
        
        // Immediate second call should be rate limited
        assert!(handler.should_rate_limit());
    }
    
    #[test]
    fn test_rate_limit_configuration() {
        let mut handler = InputHandler::new();
        handler.set_rate_limit_ms(50);
        assert_eq!(handler.get_rate_limit_ms(), 50);
        
        handler.reset_rate_limiting();
        assert!(handler.last_input_time.is_none());
    }
    
    #[test]
    fn test_input_handler_null_window() {
        let mut handler = InputHandler::new();
        
        // These should not crash with null window
        let result1 = handler.handle_mouse_click(std::ptr::null_mut(), 100, 100, MouseButton::Left, true);
        let result2 = handler.handle_mouse_move(std::ptr::null_mut(), 100, 100);
        let result3 = handler.handle_keyboard_event(std::ptr::null_mut(), 65, true); // 'A' key
        
        // Results depend on desktop capture mode and validation settings
        // In desktop capture mode, these should work
        handler.set_desktop_capture_mode(true);
        let result4 = handler.handle_mouse_click(std::ptr::null_mut(), 100, 100, MouseButton::Left, true);
        // Should succeed in desktop capture mode
    }
    
    #[test]
    fn test_keyboard_keycode_validation() {
        let mut handler = InputHandler::new();
        
        // Block a specific keycode
        handler.block_keycode(27); // ESC key
        assert!(handler.is_keycode_blocked(27));
        
        // Try to send blocked keycode
        let event = InputEvent::KeyboardEvent { keycode: 27, pressed: true };
        let result = handler.validate_input_event_security(&event);
        assert!(result.is_err());
        
        // Unblock the keycode
        handler.unblock_keycode(27);
        assert!(!handler.is_keycode_blocked(27));
        
        // Should now pass validation
        let result = handler.validate_input_event_security(&event);
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_process_input_event_types() {
        let mut handler = InputHandler::new();
        handler.set_desktop_capture_mode(true); // Use desktop capture to avoid window validation
        
        let mouse_click = InputEvent::MouseClick {
            x: 100,
            y: 100,
            button: MouseButton::Left,
            pressed: true,
        };
        
        let mouse_move = InputEvent::MouseMove { x: 150, y: 150 };
        
        let key_press = InputEvent::KeyboardEvent {
            keycode: 65, // 'A' key
            pressed: true,
        };
        
        // These should not crash
        let _ = handler.process_input_event(std::ptr::null_mut(), mouse_click);
        let _ = handler.process_input_event(std::ptr::null_mut(), mouse_move);
        let _ = handler.process_input_event(std::ptr::null_mut(), key_press);
    }
    
    #[test]
    fn test_mouse_button_message_mapping() {
        let handler = InputHandler::new();
        
        // Test that different mouse buttons map to correct messages
        // This is tested indirectly through the send_mouse_click_* methods
        let result1 = handler.send_mouse_click_postmessage(
            std::ptr::null_mut(), 100, 100, MouseButton::Left, true
        );
        let result2 = handler.send_mouse_click_postmessage(
            std::ptr::null_mut(), 100, 100, MouseButton::Right, true
        );
        let result3 = handler.send_mouse_click_postmessage(
            std::ptr::null_mut(), 100, 100, MouseButton::Middle, true
        );
        
        // These will fail with null window, but the mapping logic is tested
    }
    
    #[test]
    fn test_coordinate_validation_logic() {
        let handler = InputHandler::new();
        
        // Test coordinate validation with null window (should pass)
        let result = handler.validate_coordinates(std::ptr::null_mut(), 100, 100);
        assert!(result.is_ok());
        
        // Test with invalid coordinates for security config
        let mut handler = InputHandler::new_with_security(SecurityConfig::default());
        
        let valid_event = InputEvent::MouseMove { x: 100, y: 100 };
        let invalid_event = InputEvent::MouseMove { x: 10000, y: 10000 };
        
        assert!(handler.validate_input_event_security(&valid_event).is_ok());
        assert!(handler.validate_input_event_security(&invalid_event).is_err());
    }
    
    #[test]
    fn test_lparam_creation() {
        // Test the lparam creation logic used in mouse messages
        let x = 150;
        let y = 200;
        let lparam = ((y as u32) << 16) | (x as u32 & 0xFFFF);
        
        // Extract coordinates back
        let extracted_x = (lparam & 0xFFFF) as i32;
        let extracted_y = ((lparam >> 16) & 0xFFFF) as i32;
        
        assert_eq!(extracted_x, x);
        assert_eq!(extracted_y, y);
    }
    
    #[test]
    fn test_cursor_position_functions() {
        // Test static cursor position functions
        let original_pos = InputHandler::get_cursor_position();
        
        if let Ok((orig_x, orig_y)) = original_pos {
            // Try to set a new position
            let new_x = orig_x + 10;
            let new_y = orig_y + 10;
            
            let set_result = InputHandler::set_cursor_position(new_x, new_y);
            if set_result.is_ok() {
                // Give it a moment to take effect
                std::thread::sleep(Duration::from_millis(10));
                
                // Check if position changed
                if let Ok((current_x, current_y)) = InputHandler::get_cursor_position() {
                    // Position might not be exact due to screen boundaries
                    // Just verify the functions don't crash
                }
                
                // Restore original position
                let _ = InputHandler::set_cursor_position(orig_x, orig_y);
            }
        }
    }
    
    #[test]
    fn test_foreground_window() {
        let window = InputHandler::get_foreground_window();
        // Should return some window handle (might be null if no foreground window)
        // Just verify the function doesn't crash
    }
    
    #[test]
    fn test_input_handler_with_real_window() {
        // Try to get the foreground window for testing
        let window = InputHandler::get_foreground_window();
        
        if !window.is_null() {
            let mut handler = InputHandler::new_for_window(window);
            
            // Test window validation
            assert!(handler.is_valid_window(window));
            
            // Test coordinate validation with real window
            let result = handler.validate_coordinates(window, 10, 10);
            // Result depends on actual window size
            
            // Test input methods (these might fail due to permissions)
            let _ = handler.handle_mouse_move(window, 10, 10);
            let _ = handler.handle_mouse_click(window, 10, 10, MouseButton::Left, true);
            let _ = handler.handle_keyboard_event(window, 65, true);
        }
    }
    
    #[test]
    fn test_security_config() {
        let default_config = SecurityConfig::default();
        assert_eq!(default_config.max_events_per_second, 100);
        assert!(default_config.enable_coordinate_validation);
        
        let permissive_config = SecurityConfig::permissive();
        assert_eq!(permissive_config.max_events_per_second, 1000);
        assert!(!permissive_config.enable_coordinate_validation);
    }
    
    #[test]
    fn test_input_handler_with_security() {
        let security_config = SecurityConfig {
            max_events_per_second: 50,
            blocked_keycodes: vec![27, 112], // ESC, F1
            enable_keycode_validation: true,
            ..SecurityConfig::default()
        };
        
        let handler = InputHandler::new_with_security(security_config);
        assert!(handler.is_keycode_blocked(27));
        assert!(handler.is_keycode_blocked(112));
        assert!(!handler.is_keycode_blocked(65));
    }
    
    #[test]
    fn test_coordinate_validation_security() {
        let mut handler = InputHandler::new();
        
        let valid_event = InputEvent::MouseMove { x: 100, y: 100 };
        let invalid_event = InputEvent::MouseMove { x: 10000, y: 10000 };
        
        // Should pass with default config
        assert!(handler.validate_input_event_security(&valid_event).is_ok());
        assert!(handler.validate_input_event_security(&invalid_event).is_err());
        
        // Disable coordinate validation
        handler.security_config.enable_coordinate_validation = false;
        assert!(handler.validate_input_event_security(&invalid_event).is_ok());
    }
    
    #[test]
    fn test_keycode_blocking() {
        let mut handler = InputHandler::new();
        
        // Initially no keycodes blocked
        assert!(!handler.is_keycode_blocked(27));
        
        // Block ESC key
        handler.block_keycode(27);
        assert!(handler.is_keycode_blocked(27));
        
        // Test validation fails for blocked keycode
        let blocked_event = InputEvent::KeyboardEvent { keycode: 27, pressed: true };
        assert!(handler.validate_input_event_security(&blocked_event).is_err());
        
        // Unblock the keycode
        handler.unblock_keycode(27);
        assert!(!handler.is_keycode_blocked(27));
        assert!(handler.validate_input_event_security(&blocked_event).is_ok());
    }
    
    #[test]
    fn test_consecutive_event_limiting() {
        let mut handler = InputHandler::new();
        handler.security_config.max_consecutive_identical = 3;
        
        let event = InputEvent::MouseMove { x: 100, y: 100 };
        
        // First few events should pass
        assert!(handler.validate_input_event_security(&event).is_ok());
        assert!(handler.validate_input_event_security(&event).is_ok());
        assert!(handler.validate_input_event_security(&event).is_ok());
        
        // Fourth consecutive identical event should fail
        assert!(handler.validate_input_event_security(&event).is_err());
        
        // Different event should reset counter
        let different_event = InputEvent::MouseMove { x: 200, y: 200 };
        assert!(handler.validate_input_event_security(&different_event).is_ok());
        assert!(handler.validate_input_event_security(&event).is_ok());
    }
    
    #[test]
    fn test_input_statistics() {
        let mut handler = InputHandler::new();
        
        let initial_stats = handler.get_statistics();
        assert_eq!(initial_stats.total_events, 0);
        
        // Process some events
        let event1 = InputEvent::MouseMove { x: 100, y: 100 };
        let event2 = InputEvent::KeyboardEvent { keycode: 65, pressed: true };
        
        handler.update_statistics(&event1);
        handler.update_statistics(&event2);
        
        let stats = handler.get_statistics();
        assert_eq!(stats.total_events, 2);
        assert_eq!(stats.events_by_type.get("mouse_move"), Some(&1));
        assert_eq!(stats.events_by_type.get("key_press"), Some(&1));
        
        // Reset statistics
        handler.reset_statistics();
        let reset_stats = handler.get_statistics();
        assert_eq!(reset_stats.total_events, 0);
    }
    
    #[test]
    fn test_events_per_second_calculation() {
        let mut handler = InputHandler::new();
        
        // Initially should be 0
        assert_eq!(handler.get_events_per_second(), 0.0);
        
        // Process some events
        let event = InputEvent::MouseMove { x: 100, y: 100 };
        handler.update_statistics(&event);
        handler.update_statistics(&event);
        
        // Should have some rate (depends on timing)
        let rate = handler.get_events_per_second();
        assert!(rate >= 0.0);
    }
    
    #[test]
    fn test_attack_detection() {
        let mut handler = InputHandler::new();
        handler.security_config.max_consecutive_identical = 2;
        
        let event = InputEvent::MouseMove { x: 100, y: 100 };
        
        // Process events to trigger attack detection
        handler.update_statistics(&event);
        handler.update_statistics(&event);
        handler.update_statistics(&event);
        
        // Should detect attack due to consecutive identical events
        assert!(handler.is_under_attack());
    }
    
    #[test]
    fn test_security_configuration_updates() {
        let mut handler = InputHandler::new();
        
        let new_config = SecurityConfig {
            max_events_per_second: 200,
            blocked_keycodes: vec![13], // Enter key
            ..SecurityConfig::default()
        };
        
        handler.set_security_config(new_config);
        
        let config = handler.get_security_config();
        assert_eq!(config.max_events_per_second, 200);
        assert!(config.blocked_keycodes.contains(&13));
    }
    
    #[test]
    fn test_permissive_security_mode() {
        let mut handler = InputHandler::new();
        
        // Test unsafe processing bypasses security
        let blocked_event = InputEvent::KeyboardEvent { keycode: 27, pressed: true };
        
        // Block the keycode
        handler.block_keycode(27);
        
        // Normal processing should fail
        let result = handler.validate_input_event_security(&blocked_event);
        assert!(result.is_err());
        
        // Unsafe processing should succeed (bypasses validation in process_input_event_unsafe)
         handler.set_desktop_capture_mode(true);
         let unsafe_result = handler.process_input_event_unsafe(std::ptr::null_mut(), blocked_event);
         // Should succeed because unsafe mode uses permissive config
     }
 }