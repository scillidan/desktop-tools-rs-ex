//! Shared primitives for the dt-* tools: key combos, pure layout math, and
//! exe-relative config file helpers. Windows-first; the X11 keysym helper in
//! `keys` is inert on Windows.

pub mod config;
pub mod keys;
pub mod layout;
