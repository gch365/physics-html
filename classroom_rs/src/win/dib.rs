//! 32 位 DIB 缓冲：把 tiny-skia 的 Pixmap 交给 Windows 做逐像素 alpha 绘制 / 位图控件。

use std::ffi::c_void;

use super::sys::*;

pub struct Dib {
    pub hdc: HDC,
    pub hbmp: HBITMAP,
    pub bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Dib {
    pub fn new(w: i32, h: i32) -> Option<Self> {
        if w <= 0 || h <= 0 {
            return None;
        }
        unsafe {
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // 负高度 = 自上而下
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            bmi.bmiHeader.biSizeImage = (w * h * 4) as u32;

            let hdc = CreateCompatibleDC(std::ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let mut bits: *mut c_void = std::ptr::null_mut();
            let hbmp = CreateDIBSection(
                hdc,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if hbmp.is_null() || bits.is_null() {
                DeleteDC(hdc);
                return None;
            }
            SelectObject(hdc, hbmp);
            Some(Self {
                hdc,
                hbmp,
                bits: bits as *mut u8,
                w,
                h,
            })
        }
    }


    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            if !self.hbmp.is_null() {
                DeleteObject(self.hbmp);
            }
            if !self.hdc.is_null() {
                DeleteDC(self.hdc);
            }
        }
    }
}
