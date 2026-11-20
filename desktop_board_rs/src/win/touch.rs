//! 双指捏合缩放：触摸触点读取 + 距离比换算。
//!
//! 用 WM_TOUCH 原始触点而不是 WM_GESTURE：横幅/作业栏都是
//! `WS_EX_NOACTIVATE` 层窗口，手势识别依赖窗口激活状态，原始触点更可靠。
//!
//! 单指触摸仍由系统模拟成 WM_LBUTTON*，走原有拖动/点击逻辑；
//! 这里只在同时出现两个触点时介入缩放。

use super::sys::*;

/// 从触点数组里取前两个触点，算它们在屏幕上的距离（像素）。
/// 不足两个触点返回 None。
pub fn two_point_distance(points: &[TOUCHINPUT]) -> Option<f32> {
    if points.len() < 2 {
        return None;
    }
    let a = &points[0];
    let b = &points[1];
    let dx = (a.x - b.x) as f32 / TOUCH_COORD_TO_PIXEL;
    let dy = (a.y - b.y) as f32 / TOUCH_COORD_TO_PIXEL;
    let d = (dx * dx + dy * dy).sqrt();
    if d.is_finite() && d > 0.0 {
        Some(d)
    } else {
        None
    }
}

/// 由「起始两指距离」与「当前两指距离」算缩放倍率。
/// base_dist 过小（<10px）时返回 1.0，避免刚按下时的抖动被放大成跳变；
/// 倍率夹在 0.25~4.0，防止极端手势。
pub fn pinch_ratio(base_dist: f32, cur_dist: f32) -> f32 {
    if base_dist < 10.0 || cur_dist <= 0.0 {
        return 1.0;
    }
    (cur_dist / base_dist).clamp(0.25, 4.0)
}

/// 从 WM_TOUCH 消息里读出当前两指距离（像素）；不足两指或读取失败返回 None。
/// `wp` = 触点数（wParam），`lp` = 触摸输入句柄（lParam）。
/// 无论成功与否都会关闭句柄，避免内核对象泄漏。
pub fn read_touch_distance(wp: usize, lp: isize) -> Option<f32> {
    let count = wp;
    if count < 2 || lp == 0 {
        return None;
    }
    let mut buf = vec![TOUCHINPUT::default(); count];
    let ok = unsafe {
        GetTouchInputInfo(
            lp,
            count as u32,
            buf.as_mut_ptr(),
            std::mem::size_of::<TOUCHINPUT>() as i32,
        )
    };
    unsafe { CloseTouchInputHandle(lp) };
    if ok == 0 {
        return None;
    }
    two_point_distance(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i32, y: i32) -> TOUCHINPUT {
        TOUCHINPUT {
            x,
            y,
            ..Default::default()
        }
    }

    #[test]
    fn distance_from_two_touch_points() {
        // TOUCHINPUT 坐标单位是 1/100 px：(0,0) 与 (300,400) → 5 像素
        let d = two_point_distance(&[pt(0, 0), pt(300, 400)]).unwrap();
        assert!((d - 5.0).abs() < 0.01, "d={d}");
    }

    #[test]
    fn needs_two_points() {
        assert!(two_point_distance(&[]).is_none());
        assert!(two_point_distance(&[pt(0, 0)]).is_none());
        // 两指重合 → 距离 0 → 视为无效（避免除零）
        assert!(two_point_distance(&[pt(100, 100), pt(100, 100)]).is_none());
    }

    #[test]
    fn pinch_ratio_scales_and_clamps() {
        assert!((pinch_ratio(100.0, 200.0) - 2.0).abs() < 0.001);
        assert!((pinch_ratio(100.0, 50.0) - 0.5).abs() < 0.001);
        // 极端：夹在 0.25~4.0
        assert_eq!(pinch_ratio(10.0, 1000.0), 4.0);
        assert_eq!(pinch_ratio(100.0, 1.0), 0.25);
        // 起始距离过小 → 不缩放（防抖）
        assert_eq!(pinch_ratio(5.0, 100.0), 1.0);
    }

    /// 端到端换算：作业栏 base scale=1.0，两指从 100px 撑到 140px → 1.4
    #[test]
    fn homework_scale_from_pinch() {
        let base_scale = 1.0f32;
        let s = (base_scale * pinch_ratio(100.0, 140.0)).clamp(0.70, 1.40);
        assert!((s - 1.4).abs() < 0.001, "s={s}");
        let s2 = (1.0f32 * pinch_ratio(100.0, 60.0)).clamp(0.70, 1.40);
        assert!((s2 - 0.7).abs() < 0.001, "s2={s2}");
    }

    /// 横幅：base 宽 1000，两指从 200px 撑到 260px → 1300
    #[test]
    fn marquee_width_from_pinch() {
        let w = ((1000.0f32 * pinch_ratio(200.0, 260.0)) as i32).clamp(400, 3200);
        assert_eq!(w, 1300);
    }
}
