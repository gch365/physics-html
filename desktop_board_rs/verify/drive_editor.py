import ctypes, sys, time
from ctypes import wintypes
from PIL import ImageGrab

u = ctypes.windll.user32
k = ctypes.windll.kernel32

CB = ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)

WM_COMMAND = 0x0111
WM_CLOSE = 0x0010
SW_RESTORE = 9


def tops_of_pid(pid):
    out = []

    def cb(h, l):
        p = wintypes.DWORD()
        u.GetWindowThreadProcessId(h, ctypes.byref(p))
        if p.value == pid and u.IsWindowVisible(h):
            n = u.GetWindowTextLengthW(h)
            b = ctypes.create_unicode_buffer(n + 1)
            u.GetWindowTextW(h, b, n + 1)
            cls = ctypes.create_unicode_buffer(256)
            u.GetClassNameW(h, cls, 256)
            r = wintypes.RECT()
            u.GetWindowRect(h, ctypes.byref(r))
            out.append((h, b.value, cls.value, (r.left, r.top, r.right, r.bottom)))
        return True

    u.EnumWindows(CB(cb), 0)
    return out


def wait_window(pid, cls, timeout=10):
    t0 = time.time()
    while time.time() - t0 < timeout:
        for h, title, c, rect in tops_of_pid(pid):
            if c == cls:
                return h
        time.sleep(0.2)
    return None


def shot(name):
    im = ImageGrab.grab()
    im.save(name)
    print("saved", name, im.size)


def main():
    pid = int(sys.argv[1])
    steps = {
        "settings": (801, "BoardSettingsRust", "shot_ed_settings.png"),
        "homework": (802, "BoardHwRust", "shot_ed_homework.png"),
        "duty": (803, "BoardDutyRust", "shot_ed_duty.png"),
        "tasks": (804, "BoardTasksRust", "shot_ed_tasks.png"),
    }
    which = sys.argv[2:] or list(steps)
    main_hwnd = wait_window(pid, "BoardEditorRust")
    if not main_hwnd:
        print("main window not found")
        return
    u.ShowWindow(main_hwnd, SW_RESTORE)
    u.SetForegroundWindow(main_hwnd)
    time.sleep(0.6)
    for name in which:
        cid, cls, png = steps[name]
        u.PostMessageW(main_hwnd, WM_COMMAND, cid, 0)
        h = wait_window(pid, cls)
        if not h:
            print("dialog not found:", name, cls)
            continue
        time.sleep(0.8)
        shot(png)
        u.PostMessageW(h, WM_CLOSE, 0, 0)
        time.sleep(0.6)
    # 最后再截一张主窗口
    u.SetForegroundWindow(main_hwnd)
    time.sleep(0.5)
    shot("shot_ed_main.png")


main()
