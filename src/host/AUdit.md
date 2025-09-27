The provided code is a complex Rust implementation of a Windows input handler for an HVNC (Hidden Virtual Network Computing) system. It uses the Windows API to handle mouse and keyboard input events with different injection strategies (SendInput, SendMessage, PostMessage). However, there are a few issues in the code that need fixing, such as duplicate function definitions, incomplete implementations, and potential logical errors. Below, I'll address the key issues and provide a corrected version of the problematic sections, ensuring the code is consistent and functional.

### Identified Issues
1. **Duplicate `process_input_event` Definition**:
   - The `process_input_event` function is defined twice in the code (lines 601-618 and lines 1084-1100). This will cause a compilation error due to duplicate definitions. The second definition seems to be a simplified version, but both cannot coexist.

2. **Duplicate `handle_keyboard_event` Definition**:
   - The `handle_keyboard_event` function is also defined twice (lines 557-604 and lines 620-666). This will cause another compilation error.

3. **Incomplete `update_statistics` Implementation**:
   - The `update_statistics` function is defined twice (lines 550-563 and elsewhere). The second definition in the provided snippet is incomplete and incorrectly placed within the `handle_mouse_move` function.

4. **Potential Logical Issues**:
   - The `update_statistics` function uses different keys for event types in its two definitions (`mouse_click`, `mouse_move`, `keyboard` vs. `{:?}` stringified event). This inconsistency could lead to unreliable statistics tracking.
   - The `send_mouse_click_sendinput`, `send_mouse_click_sendmessage`, and `send_mouse_click_postmessage` methods are defined but not used in the first `process_input_event`. The first definition seems to ignore these methods, which could be a logical error.
   - The `InputHandler` struct's `input_strategy` and `desktop_capture_mode` fields are initialized but not always used consistently across methods.

5. **Missing Implementation for `send_mouse_click_sendinput`, `send_mouse_click_sendmessage`, `send_mouse_click_postmessage`**:
   - These methods are referenced in `handle_mouse_click` but not fully implemented in the provided snippet before the test section. They appear later, which suggests they might be out of order or incomplete in the context of the earlier code.

6. **Test Code Issues**:
   - The test cases assume a Windows environment and may fail in non-Windows or headless environments. While this isn't a bug, it could be improved with better error handling or environment checks.
   - The `test_input_handler_with_real_window` test launches Notepad and performs input operations, but it doesn't handle cleanup robustly (e.g., ensuring the desktop is closed).

7. **Security Validation Inconsistency**:
   - The `validate_input_event_security` method checks for consecutive identical events but doesn't reset the counter when a different event is received, which could lead to false positives in spam detection.

8. **Logging Initialization**:
   - The `init_server_debug_log` function is called in every `InputHandler::new` variant, which could lead to multiple file openings. It should be initialized once globally.

### Fixes and Improvements
Below, I'll provide corrected versions of the problematic sections and suggest improvements to make the code more robust and maintainable. I'll focus on resolving the duplicate definitions, ensuring consistent statistics tracking, and improving the overall structure. I'll also ensure that the code compiles by removing redundancies and fixing logical issues.

#### 1. Merge and Fix `process_input_event`
The duplicate `process_input_event` definitions need to be merged into a single, consistent implementation. The second definition (lines 1084-1100) seems to be a simplified version, so we'll keep the more detailed one (lines 601-618) and ensure it uses the appropriate input methods.

#### 2. Merge and Fix `handle_keyboard_event`
The duplicate `handle_keyboard_event` definitions will be merged, keeping the more complete version (lines 557-604) and ensuring it integrates properly with the dynamic input strategy.

#### 3. Fix `update_statistics`
The `update_statistics` function will be consolidated into a single, consistent implementation that uses fixed event type strings (`mouse_click`, `mouse_move`, `keyboard_event`) to avoid inconsistencies.

#### 4. Ensure Consistent Input Strategy Usage
The `handle_mouse_click` and `handle_mouse_move` methods already use the dynamic input strategy correctly, but we'll ensure that all input methods (`send_mouse_click_*`, `send_mouse_move_*`, `send_keyboard_*`) are properly called and implemented.

#### 5. Improve Logging Initialization
Move the `init_server_debug_log` call to a `lazy_static` initializer to ensure it's called only once.

#### 6. Fix Security Validation
Update `validate_input_event_security` to reset the consecutive event counter when a different event is received.

#### Corrected Code
Below is the corrected version of the problematic sections, with only the relevant parts shown to avoid duplicating the entire code. I'll include the merged `process_input_event`, `handle_keyboard_event`, `update_statistics`, and related methods, along with the improved logging initialization. The rest of the code (e.g., test cases, helper methods like `send_mouse_click_*`) appears correct but will be referenced to ensure consistency.

```rust
use crate::{HvncError, Result};
use crate::common::{InputEvent, MouseButton};
use crate::host::WindowHandle;
use std::mem;
use winapi::shared::windef::{POINT, RECT};
use winapi::um::winuser::{
    PostMessageA, GetWindowRect, SetThreadDesktop, GetThreadDesktop,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, 
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_KEYDOWN, WM_KEYUP,
    GetCursorPos, SetCursorPos, GetForegroundWindow, SendInput, 
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, KEYEVENTF_KEYUP, 
    MOUSEEVENTF_ABSOLUTE, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, 
    SendMessageA, SetForegroundWindow, AttachThreadInput, GetWindowThreadProcessId,
    IsWindow, IsWindowVisible, MapVirtualKeyA, MAPVK_VK_TO_VSC, KEYEVENTF_SCANCODE
};
use winapi::um::processthreadsapi::GetCurrentThreadId;
use winapi::um::errhandlingapi::GetLastError;
use log::{debug, warn, info, error};
use std::time::{Duration, Instant};
use std::collections::HashMap;
use lazy_static::lazy_static;
use std::fs::OpenOptions;
use std::io::{Write, BufWriter};
use std::sync::Mutex;
use chrono::Utc;

// Server-side debug logging
lazy_static! {
    static ref DEBUG_LOGGER: Mutex<Option<BufWriter<std::fs::File>>> = {
        let mut logger = None;
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("input_debug_server.log")
        {
            logger = Some(BufWriter::new(file));
        }
        Mutex::new(logger)
    };
}

fn log_server_debug(message: &str) {
    let timestamp = Utc::now().format("%H:%M:%S%.3f");
    let formatted = format!("[{}] SERVER: {}", timestamp, message);
    
    // Print to console
    println!("🔧 {}", formatted);
    
    // Write to file
    if let Ok(mut logger) = DEBUG_LOGGER.lock() {
        if let Some(ref mut writer) = *logger {
            let _ = writeln!(writer, "{}", formatted);
            let _ = writer.flush();
        }
    }
}

// ... (InputMethod, InputStrategy, SecurityConfig, InputStatistics structs remain unchanged)

impl InputHandler {
    // ... (Other methods like new, new_for_desktop_capture, etc., remain unchanged)

    /// Update input statistics
    fn update_statistics(&mut self, event: &InputEvent) {
        self.input_statistics.total_events += 1;

        // Update event type statistics
        let event_type = match event {
            InputEvent::MouseClick { .. } => "mouse_click",
            InputEvent::MouseMove { .. } => "mouse_move",
            InputEvent::KeyboardEvent { .. } => "keyboard_event",
        };

        *self.input_statistics.events_by_type
            .entry(event_type.to_string())
            .or_insert(0) += 1;

        // Update last event for duplicate detection
        if self.input_statistics.last_event.as_ref() != Some(event) {
            self.input_statistics.consecutive_identical_count = 1;
        } else {
            self.input_statistics.consecutive_identical_count += 1;
        }
        self.input_statistics.last_event = Some(event.clone());
    }

    /// Validate input event for security
    fn validate_input_event_security(&mut self, event: &InputEvent) -> Result<()> {
        // Check for consecutive identical events (spam protection)
        if self.input_statistics.consecutive_identical_count > self.security_config.max_consecutive_identical {
            self.input_statistics.validation_rejected_events += 1;
            return Err(HvncError::Connection(format!(
                "Too many consecutive identical events: {}", 
                self.input_statistics.consecutive_identical_count
            )));
        }

        // Validate based on event type
        match event {
            InputEvent::MouseClick { x, y, button: _, pressed: _ } | InputEvent::MouseMove { x, y } => {
                if self.security_config.enable_coordinate_validation {
                    if *x < self.security_config.min_coordinate_value || 
                       *x > self.security_config.max_coordinate_value ||
                       *y < self.security_config.min_coordinate_value || 
                       *y > self.security_config.max_coordinate_value {
                        self.input_statistics.validation_rejected_events += 1;
                        return Err(HvncError::Connection(format!(
                            "Coordinates ({}, {}) outside allowed range ({} to {})", 
                            x, y, self.security_config.min_coordinate_value, 
                            self.security_config.max_coordinate_value
                        )));
                    }
                }
            }
            InputEvent::KeyboardEvent { keycode, pressed: _ } => {
                if self.security_config.enable_keycode_validation {
                    // Check if keycode is blocked
                    if self.security_config.blocked_keycodes.contains(keycode) {
                        self.input_statistics.validation_rejected_events += 1;
                        return Err(HvncError::Connection(format!(
                            "Keycode {} is blocked for security reasons", keycode
                        )));
                    }
                    
                    // Validate keycode range
                    if *keycode > 255 {
                        self.input_statistics.validation_rejected_events += 1;
                        return Err(HvncError::Connection(format!(
                            "Invalid keycode: {}", keycode
                        )));
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Handle a keyboard event using dynamic input selection
    pub fn handle_keyboard_event(
        &mut self,
        window: WindowHandle,
        keycode: u32,
        pressed: bool,
    ) -> Result<()> {
        if self.should_rate_limit() {
            debug!("Rate limiting keyboard event");
            return Ok(());
        }
        
        debug!("Handling keyboard event: keycode {} pressed: {}", keycode, pressed);
        
        // Validate keycode (Windows virtual key codes are 0-255)
        if keycode > 255 {
            return Err(HvncError::Connection(
                format!("Invalid keycode: {}", keycode)
            ));
        }
        
        // 🔍 HVNC Enhancement: Dynamic input method selection
        let target_window = if self.desktop_capture_mode {
            // In desktop capture mode, use foreground window
            unsafe { GetForegroundWindow() }
        } else if self.target_desktop.is_some() {
            // For hidden desktop context, find the focused window
            unsafe { GetForegroundWindow() }
        } else {
            window
        };
        
        // Switch to hidden desktop context for input
        self.switch_to_target_desktop()?;
        
        info!("⌨️ HVNC Keyboard Input: Sending keycode {} {} to window {:?}", 
              keycode, if pressed { "DOWN" } else { "UP" }, target_window);
        
        // 🚀 Dynamic Input Strategy Selection
        let result = if self.desktop_capture_mode {
            // Desktop capture mode: Use SendInput for global input
            self.send_keyboard_sendinput(keycode, pressed)
        } else if self.target_desktop.is_some() {
            // Hidden desktop context: Use SendMessage for precise targeting
            if !target_window.is_null() && self.is_valid_window(target_window) {
                // Attach to window thread for better input delivery
                self.attach_to_window_thread(target_window)?;
                let result = self.send_keyboard_sendmessage(target_window, keycode, pressed);
                self.detach_from_window_thread(target_window);
                result
            } else {
                self.send_keyboard_sendinput(keycode, pressed)
            }
        } else if !target_window.is_null() && self.is_valid_window(target_window) {
            // Regular window targeting: Use PostMessage for async delivery
            self.send_keyboard_postmessage(target_window, keycode, pressed)
        } else {
            // Fallback to SendInput
            self.send_keyboard_sendinput(keycode, pressed)
        };
        
        // Restore original desktop context
        self.restore_original_desktop()?;
        
        match result {
            Ok(_) => {
                debug!("✅ HVNC Keyboard event sent successfully using dynamic strategy");
                Ok(())
            }
            Err(e) => {
                error!("❌ Failed to send keyboard event: {}", e);
                Err(e)
            }
        }
    }

    /// Process a generic input event with security validation
    pub fn process_input_event(
        &mut self,
        window: WindowHandle,
        event: InputEvent,
    ) -> Result<()> {
        log_server_debug(&format!("RECEIVED INPUT EVENT: {:?}", event));
        debug!("Processing input event: {:?}", event);
        
        // Check rate limiting first
        if self.should_rate_limit() {
            log_server_debug("Input event RATE LIMITED");
            debug!("Input event rate limited");
            return Ok(()); // Silently drop rate-limited events
        }
        
        // Validate input event for security
        self.validate_input_event_security(&event)?;
        
        // Update statistics
        self.update_statistics(&event);
        
        // Process the event
        match event {
            InputEvent::MouseClick { x, y, button, pressed } => {
                log_server_debug(&format!("Processing MOUSE CLICK: {:?} at ({},{}) {}", 
                                        button, x, y, if pressed { "DOWN" } else { "UP" }));
                self.handle_mouse_click(window, x, y, button, pressed)?;
            }
            InputEvent::MouseMove { x, y } => {
                log_server_debug(&format!("Processing MOUSE MOVE: ({},{})", x, y));
                self.handle_mouse_move(window, x, y)?;
            }
            InputEvent::KeyboardEvent { keycode, pressed } => {
                log_server_debug(&format!("Processing KEYBOARD: keycode {} {}", 
                                        keycode, if pressed { "DOWN" } else { "UP" }));
                self.handle_keyboard_event(window, keycode, pressed)?;
            }
        }
        
        log_server_debug("Input event processed successfully");
        Ok(())
    }
}

// ... (Rest of the InputHandler methods like send_mouse_click_*, send_mouse_move_*, etc., remain unchanged)

// Test module remains unchanged but can be improved for robustness
#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::desktop::create_hidden_desktop;
    use crate::host::launcher::{launch_app_in_desktop, find_window};
    use std::time::Duration;
    use std::ptr;
    
    // ... (Existing test cases remain unchanged)
}
```

### Key Changes and Explanations
1. **Removed Duplicate Definitions**:
   - Merged the two `process_input_event` functions into a single implementation that uses the detailed version (lines 601-618) and ensures it calls the appropriate `send_*` methods via `handle_mouse_click`, `handle_mouse_move`, and `handle_keyboard_event`.
   - Merged the two `handle_keyboard_event` functions into a single implementation, keeping the more complete version (lines 557-604).

2. **Fixed `update_statistics`**:
   - Consolidated the `update_statistics` function to use consistent event type strings (`mouse_click`, `mouse_move`, `keyboard_event`) instead of `{:?}` stringification.
   - Added logic to reset `consecutive_identical_count` when a different event is received, improving spam detection accuracy.

3. **Improved Logging Initialization**:
   - Moved the `init_server_debug_log` logic into the `lazy_static` initializer for `DEBUG_LOGGER`, ensuring the log file is opened only once when the module is first used.

4. **Enhanced Security Validation**:
   - Updated `validate_input_event_security` to integrate with the corrected `update_statistics` logic, ensuring the consecutive event counter is properly managed.

5. **Maintained Input Strategy Logic**:
   - Ensured that `handle_mouse_click`, `handle_mouse_move`, and `handle_keyboard_event` correctly use the dynamic input strategy (`SendInput`, `SendMessage`, `PostMessage`) based on the context (desktop capture mode, hidden desktop, or window targeting).

6. **Test Module**:
   - The test module remains unchanged, as it is mostly correct. However, I recommend adding environment checks to skip tests that require a real Windows desktop in headless environments. This can be done by checking for a display environment variable or using a feature flag.

### Additional Recommendations
- **Error Handling in Tests**:
  Add checks for the test environment to skip tests requiring a graphical desktop if running in a headless environment:
  ```rust
  #[test]
  fn test_input_handler_with_real_window() {
      if std::env::var("DISPLAY").is_err() {
          println!("Skipping real window test in headless environment");
          return;
      }
      // ... rest of the test
  }
  ```

- **Robust Desktop Cleanup**:
  In `test_input_handler_with_real_window`, ensure the hidden desktop is closed properly:
  ```rust
  #[test]
  fn test_input_handler_with_real_window() {
      match create_hidden_desktop("test_input_handler") {
          Ok(desktop) => {
              // ... existing test logic
              unsafe { CloseDesktop(desktop.handle()) };
          }
          Err(_) => {
              println!("Warning: Could not create test desktop, skipping real window test");
          }
      }
  }
  ```

- **Documentation**:
  Add more detailed documentation for public methods, especially `process_input_event`, `handle_mouse_click`, `handle_mouse_move`, and `handle_keyboard_event`, to clarify their usage in an HVNC context.

- **Performance Optimization**:
  Consider adding a cache for `GetSystemMetrics(SM_CXSCREEN)` and `SM_CYSCREEN)` calls in `send_mouse_click_sendinput` and `send_mouse_move_sendinput`, as these values are unlikely to change during execution.

### Testing the Fixes
To verify the fixes, compile and run the code in a Windows environment with the necessary dependencies (`winapi`, `lazy_static`, `chrono`, `log`). Run the test suite using:
```bash
cargo test
```
Ensure that the tests pass, especially `test_input_handler_with_real_window`, which requires a graphical environment. If running in CI, consider using a Windows runner or mocking the Windows API calls for headless testing.

### Conclusion
The corrected code resolves the duplicate definitions, ensures consistent statistics tracking, and improves logging initialization. The input handling logic remains robust, supporting dynamic input strategies for HVNC applications. 