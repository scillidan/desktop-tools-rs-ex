//! One frame per virtual desktop, owned by the single global-hotkey process.

use anyhow::{Context, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use crate::config::Config;
use crate::frame;

pub struct Frames {
    manager: IVirtualDesktopManager,
    items: Vec<HWND>,
}

impl Frames {
    pub fn new(cfg: &Config) -> Result<Self> {
        let manager: IVirtualDesktopManager = unsafe {
            CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_ALL)
                .context("cannot create VirtualDesktopManager")?
        };
        let first = create(cfg)?;
        crate::set_frame_hwnd(first);
        Ok(Self {
            manager,
            items: vec![first],
        })
    }

    /// Reuse the current desktop's frame, creating it on the first hotkey
    /// received on that desktop. Public desktop APIs also tell us when the
    /// shell cloaks a frame on another desktop.
    pub fn current(&mut self, cfg: &Config) -> Result<HWND> {
        self.retain();
        let mut current = None;
        for &hwnd in &self.items {
            if unsafe { self.manager.IsWindowOnCurrentVirtualDesktop(hwnd)? }.as_bool() {
                if current.is_none() {
                    current = Some(hwnd);
                } else {
                    // Closing a desktop moves its windows to another desktop;
                    // collapse those arrivals back into the one-frame rule.
                    unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd)? };
                }
            }
        }
        if let Some(hwnd) = current {
            crate::set_frame_hwnd(hwnd);
            return Ok(hwnd);
        }
        self.retain();
        let hwnd = create(cfg)?;
        self.items.push(hwnd);
        crate::set_frame_hwnd(hwnd);
        Ok(hwnd)
    }

    pub fn all(&mut self) -> &[HWND] {
        self.retain();
        &self.items
    }

    fn retain(&mut self) {
        self.items
            .retain(|&hwnd| unsafe { IsWindow(Some(hwnd)) }.as_bool());
    }
}

fn create(cfg: &Config) -> Result<HWND> {
    unsafe {
        frame::create(
            cfg.frame_w,
            cfg.frame_h,
            cfg.work_area,
            cfg.show_hint,
            cfg.font.clone(),
            cfg.ocr_scale,
            cfg.ocr_language.clone(),
        )
    }
}
