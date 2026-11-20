# -*- coding: utf-8 -*-
"""用 Python 原版渲染同一时刻的大屏图，用于和 Rust 版做 1:1 视觉比对。"""
import sys, importlib.util
from datetime import datetime

SRC = r"C:\Users\gchui\xwechat_files\wxid_0sv1yhmo7k8y21_780f\temp\RWTemp\2026-11\9e20f478899dc29eb19741386f9343c8\desktop_board.py"
spec = importlib.util.spec_from_file_location("db", SRC)
db = importlib.util.module_from_spec(spec)
sys.modules["db"] = db
spec.loader.exec_module(db)

now = datetime.strptime(sys.argv[1], "%Y-%m-%d %H:%M") if len(sys.argv) > 1 else datetime.now()
out = sys.argv[2] if len(sys.argv) > 2 else "preview_py.png"

cfg = db.load_app_config()
raw_name = cfg.get("students_class") or cfg.get("current_class") or ""
board = db.load_board(raw_name)
api = db.load_schedule_api()
courses = db.load_courses(cfg.get("current_class", ""))
st = db.compute_status(api, cfg.get("work_mode", "summer"), courses, now)
notice = db.load_board_notice()
img = db.render_board_image(board, st, notice, now, 1920, 1080)
img.convert("RGB").save(out)
print("saved", out, img.size, "|", st.get("state"), st.get("cd_text"))
