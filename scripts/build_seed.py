#!/usr/bin/env python3
"""把学校下发的 Excel 名单 / 全校课表转换成点名程序可读取的种子数据。

数据源（本机文件，不入库）：
  - 赣州中学高二学生名单.xlsx    Sheet1: 班号 | 姓名 | (空列)，一行一名
  - 赣州中学高二上课表(正式).xls 全校课表3: 行=班级，列=星期×节次（宽表）

产物（都在 ~/.RollCallData/，**不进 git**）：
  seed/seed.json                     一次性上传源；由 upload_class_data.py 拆成
                                     按班 JSON 传到私有仓库，程序运行时不读它
  config.json / schedules/*.json     --apply 时一次性导入全部班级（仅管理员本机）

用法：
  python scripts/build_seed.py                 # 只生成 seed.json
  python scripts/build_seed.py --apply        # 生成 + 导入（自动先备份），发布版勿用
  python scripts/build_seed.py --dry-run      # 只打印统计，不写任何文件
  python scripts/upload_class_data.py --apply  # 生成后按班上传到私有仓库

依赖：openpyxl（.xlsx）、xlrd（.xls）。只用于离线转换，程序运行时不需要 Python。
学生姓名属于个人信息：本脚本产物只留在管理员构建机 / 私有仓库，
绝不进源码仓库、绝不随发布包分发、绝不上传到 kejian 发布通道。
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import shutil
import sys
from pathlib import Path

DATA_DIR = Path.home() / ".RollCallData"
DESKTOP = Path.home() / "Desktop"
DEFAULT_STUDENTS = DESKTOP / "赣州中学高二学生名单.xlsx"
DEFAULT_SCHEDULE = DESKTOP / "赣州中学高二上课表(正式).xls"
SEED_FILE = DATA_DIR / "seed" / "seed.json"

SCHOOL = "赣州中学"
YEAR = "2025"  # 入学年份；2026~2027 学年正好是高二
MAX_PERIOD = 12  # 只导入 1~12 节（跳过早读、班会/活动/晚自习以外的列不动）
WEEKDAY_LABELS = {"星期一": 0, "星期二": 1, "星期三": 2, "星期四": 3, "星期五": 4}
PLACEHOLDERS = {"", "-", "—", "无", "空", "/"}


class SeedError(RuntimeError):
    pass


def cell_text(v) -> str:
    """xlrd / openpyxl 的单元格值统一成干净字符串。"""
    if v is None:
        return ""
    if isinstance(v, float) and v.is_integer():
        return str(int(v))
    return str(v).strip()


def class_number(v) -> str | None:
    """'高二(1)' / 1 / 1.0 / '01' → '1'；取不到数字返回 None。"""
    m = re.search(r"\d+", cell_text(v))
    return str(int(m.group(0))) if m else None


def read_students(path: Path) -> dict[str, list[str]]:
    try:
        import openpyxl
    except ImportError as e:
        raise SeedError("缺少 openpyxl，请先 python -m pip install openpyxl") from e
    if not path.exists():
        raise SeedError(f"找不到学生名单：{path}")
    ws = openpyxl.load_workbook(path, read_only=True, data_only=True).worksheets[0]
    out: dict[str, list[str]] = {}
    for row in ws.iter_rows(values_only=True):
        cn = class_number(row[0] if len(row) > 0 else None)
        name = cell_text(row[1] if len(row) > 1 else None)
        # 表头行 / 噪声：班号列为空、姓名是「姓名」或纯数字学号
        if cn is None or not name or name in ("姓名", "学生姓名") or name.isdigit():
            continue
        out.setdefault(cn, []).append(name)
    if not out:
        raise SeedError(f"没有从 {path.name} 解析出任何学生")
    return out


def read_schedule(path: Path) -> dict[str, list[dict]]:
    try:
        import xlrd
    except ImportError as e:
        raise SeedError("缺少 xlrd，请先 python -m pip install xlrd") from e
    if not path.exists():
        raise SeedError(f"找不到课表：{path}")
    sh = xlrd.open_workbook(str(path)).sheet_by_index(0)
    if sh.nrows < 4:
        raise SeedError("课表行数不足，缺少表头或班级行")
    week_header = sh.row_values(1)
    period_header = sh.row_values(2)

    # 星期是合并单元格：向左填充标签，得到每一列属于第几天。
    day_at: list[str | None] = []
    cur: str | None = None
    for c in range(sh.ncols):
        label = cell_text(week_header[c]) if c < len(week_header) else ""
        if label:
            cur = label
        day_at.append(cur)

    # 只保留「星期一~星期五」且节次标签是 1..MAX_PERIOD 的列。
    cols: list[tuple[int, int]] = []  # (列号, weekday, period)
    for c in range(1, sh.ncols):
        day = day_at[c]
        if day not in WEEKDAY_LABELS:
            continue  # 早读前的空列、星期日整块都跳过
        p = cell_text(period_header[c]) if c < len(period_header) else ""
        if not p.isdigit():
            continue  # 「早读」不占 period，避免与作息节次错位
        period = int(p)
        if 1 <= period <= MAX_PERIOD:
            cols.append((c, WEEKDAY_LABELS[day], period))

    out: dict[str, list[dict]] = {}
    for r in range(3, sh.nrows):
        cn = class_number(sh.cell_value(r, 0))
        if cn is None:
            continue
        slots: list[dict] = []
        for c, weekday, period in cols:
            course = cell_text(sh.cell_value(r, c))
            if course in PLACEHOLDERS:
                continue
            slots.append({"weekday": weekday, "period": period, "course": course})
        slots.sort(key=lambda s: (s["weekday"], s["period"]))
        out[cn] = slots
    if not out:
        raise SeedError(f"没有从 {path.name} 解析出任何班级课表")
    return out


def build_payload(
    students: dict[str, list[str]],
    schedules: dict[str, list[dict]],
    students_name: str,
    schedule_name: str,
) -> dict:
    keys = sorted(set(students) | set(schedules), key=lambda x: int(x))
    return {
        "version": 1,
        "school": SCHOOL,
        "year": YEAR,
        "generated_at": dt.datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
        "sources": {"students": students_name, "schedule": schedule_name},
        # 程序 ClassEntry::build_file_name / build_display_name 的规则
        "file_name_template": f"{SCHOOL}_{YEAR}级{{NN}}班.txt",
        "classes": {
            cn: {"students": students.get(cn, []), "schedule": schedules.get(cn, [])}
            for cn in keys
        },
    }


def write_json(path: Path, data) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def backup_runtime() -> Path:
    stamp = dt.datetime.now().strftime("%Y%m%d_%H%M%S")
    dest = DATA_DIR / f"backup_seed_{stamp}"
    dest.mkdir(parents=True, exist_ok=False)
    cfg = DATA_DIR / "config.json"
    if cfg.exists():
        shutil.copy2(cfg, dest / "config.json")
    sched = DATA_DIR / "schedules"
    if sched.is_dir():
        shutil.copytree(sched, dest / "schedules")
    return dest


def apply_to_runtime(payload: dict) -> dict:
    """把种子导入 config.json 与 schedules/：已有班级只更新 students，其余字段原样保留。"""
    cfg_path = DATA_DIR / "config.json"
    if not cfg_path.exists():
        raise SeedError(f"找不到 {cfg_path}，请先运行一次主程序生成配置")
    cfg = json.loads(cfg_path.read_text(encoding="utf-8"))
    backup = backup_runtime()

    entries = {c.get("file_name", ""): c for c in cfg.get("class_list", [])}
    added = refreshed = 0
    for cn, item in payload["classes"].items():
        nn = f"{int(cn):02d}"
        file_name = f"{SCHOOL}_{YEAR}级{nn}班.txt"
        names = item["students"]
        if file_name in entries:
            entries[file_name]["students"] = names
            refreshed += 1
        else:
            cfg.setdefault("class_list", []).append({
                "school": SCHOOL,
                "year": YEAR,
                "class_num": cn,
                "display_name": f"{SCHOOL}{YEAR}级{int(cn)}班",
                "file_name": file_name,
                "students": names,
            })
            entries[file_name] = cfg["class_list"][-1]
            added += 1

        slots = item["schedule"]
        write_json(
            DATA_DIR / "schedules" / f"class_{file_name}.json",
            {"class": file_name, "schedule": slots},
        )
    write_json(cfg_path, cfg)
    return {"added": added, "refreshed": refreshed, "backup": str(backup)}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--students", type=Path, default=DEFAULT_STUDENTS)
    ap.add_argument("--schedule", type=Path, default=DEFAULT_SCHEDULE)
    ap.add_argument("--out", type=Path, default=SEED_FILE)
    ap.add_argument("--apply", action="store_true", help="同时导入 config.json 与课表（先备份）")
    ap.add_argument("--dry-run", action="store_true", help="只打印统计，不写任何文件")
    args = ap.parse_args()

    try:
        students = read_students(args.students)
        schedules = read_schedule(args.schedule)
        payload = build_payload(
            students, schedules, args.students.name, args.schedule.name
        )
        total = sum(len(v) for v in students.values())
        slots = sum(len(v) for v in schedules.values())
        sizes = [len(v) for v in students.values()]
        print(f"班级 {len(payload['classes'])} 个 | 学生 {total} 人 "
              f"(每班 {min(sizes)}~{max(sizes)}) | 课表格子 {slots} 个")
        for cn in sorted(payload["classes"], key=int)[:3]:
            item = payload["classes"][cn]
            print(f"  {cn} 班: {len(item['students'])} 人, {len(item['schedule'])} 节课, "
                  f"前 3 人 {item['students'][:3]}")
        if args.dry_run:
            print("dry-run：未写入任何文件")
            return 0
        write_json(args.out, payload)
        print(f"种子已写入 {args.out} ({args.out.stat().st_size // 1024} KB)")
        if args.apply:
            r = apply_to_runtime(payload)
            print(f"已导入：新增班级 {r['added']} 个，刷新名单 {r['refreshed']} 个，"
                  f"课表 {len(payload['classes'])} 份")
            print(f"原配置已备份到 {r['backup']}")
        return 0
    except SeedError as e:
        print(f"[build_seed] 失败: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
