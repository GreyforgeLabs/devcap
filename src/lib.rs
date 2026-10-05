//! devcap — development environment capability scanner.
//!
//! Detects installed developer tools on `PATH`, probes their versions with
//! bounded, sandboxed subprocesses, and reports text, JSON, or Markdown.
//! This crate is the Rust rewrite of the original Python implementation and
//! keeps its command-line behavior and output formats.

// All `unsafe` is confined to `sys` (small, documented libc calls).
#![deny(unsafe_code)]

pub mod argparse;
pub mod cli;
pub mod formatters;
pub mod json;
pub mod process;
pub mod profile;
pub mod pycompat;
pub mod registry;
pub mod safe_text;
pub mod scanner;
pub mod shlex;
#[allow(unsafe_code)]
mod sys;
pub mod toml;
mod unicode_tables;

pub use registry::{CATEGORIES, ToolDef, registry};
pub use scanner::{ScanResult, ServiceResult, ToolResult, scan_tools};
