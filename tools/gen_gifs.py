# 用本机 Edge(Chromium) 无头渲染物理实验 HTML，截帧后用 ffmpeg 合成循环 GIF
# 用法：cd 仓库根目录后执行 python tools/gen_gifs.py
import os, subprocess, sys
from playwright.sync_api import sync_playwright

# 若本机 Edge 路径不同，修改此处
EDGE = r"C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT  = os.path.join(ROOT, "gifs")
FRM  = os.path.join(OUT, "_frames")
os.makedirs(OUT, exist_ok=True)

# ===== 在这里增删 GIF 实验条目（featured 顺序即九宫格顺序）=====
# driver: "play" 需点 #play 启动；"auto" 自动循环；"none" 不动
# reemit: 每隔几秒点一次 #emit（用于需持续发射粒子的实验）
EXPS = [
    {"key":"dengshiyuan", "name":"等时圆模型",
     "file": os.path.join(ROOT,"exp/f01/等时圆分析.html"),
     "driver":"play", "fps":12, "dur":6.0},
    {"key":"weifen", "name":"匀变速直线运动的微分关系",
     "file": os.path.join(ROOT,"exp/f01/（1）匀变速微分-仿写.html"),
     "driver":"slider", "slider":"N", "lo":3, "hi":200, "fps":12, "dur":4.0, "w":320,"h":170,"colors":32},
    {"key":"dingjiao", "name":"动态平衡：定角三力平衡",
     "file": os.path.join(ROOT,"exp/f01/（47）动态平衡定角三力平衡-仿写.html"),
     "driver":"slider", "slider":"pos", "lo":0.06, "hi":0.94, "fps":12, "dur":4.0},
    {"key":"pingpao", "name":"平抛运动",
     "file": os.path.join(ROOT,"exp/f01/（5）平抛运动-仿写.html"),
     "driver":"play", "fps":12, "dur":5.0},

    {"key":"tanhuang", "name":"弹簧物块动量守恒",
     "file": os.path.join(ROOT,"exp/f01/（25）弹簧物块动量守恒-仿写.html"),
     "driver":"play", "fps":12, "dur":5.0},
    {"key":"renship", "name":"动量守恒·人船模型（悬球摆球）",
     "file": os.path.join(ROOT,"exp/f01/（49）动量守恒人船模型-仿写.html")+"?scene=1",
     "driver":"auto", "fps":12, "dur":6.0},
    {"key":"weixing", "name":"同步卫星发射（变轨）",
     "file": os.path.join(ROOT,"exp/f01/同步卫星的发射(变轨).html"),
     "driver":"auto", "fps":12, "dur":8.0},
    {"key":"cifocus", "name":"磁聚焦与磁发散",
     "file": os.path.join(ROOT,"exp/f01/（57）磁聚焦磁发散-仿写.html"),
     "driver":"auto", "reemit":2.5, "fps":12, "dur":7.0},
    {"key":"shuangfeng", "name":"电子双缝干涉：概率波与统计分布",
     "file": os.path.join(ROOT,"exp/f01/电子双缝干涉.html"),
     "driver":"auto", "fps":12, "dur":8.0, "pad":"#0b1020"},
]

HIDE_CSS = (
    "#topbar,.params,#control-panel,.btns,.toggles,.readout,.caption,header,nav,"
    "footer,.tip{display:none!important}"
    "body{margin:0!important;background:#ffffff!important}"
    "#main{padding:0!important;gap:10px!important;justify-content:center}"
)

def triangle(frac):
    f = frac * 2.0
    return f if f < 1 else 2 - f

def make_gif(fd, out_path, fps, w=360, h=240, colors=64, pad="white"):
    pal = os.path.join(fd, "palette.png")
    subprocess.run(["ffmpeg","-y","-framerate",str(fps),
                    "-i",os.path.join(fd,"%03d.png"),
                    "-vf",f"scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,palettegen=max_colors={colors}",
                    pal], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["ffmpeg","-y","-framerate",str(fps),
                    "-i",os.path.join(fd,"%03d.png"),"-i",pal,
                    "-lavfi",f"[0:v]scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos[x];"
                              f"[x]pad={w}:{h}:({w}-iw)/2:({h}-ih)/2:color={pad}[y];"
                              "[y][1:v]paletteuse=dither=bayer",
                    "-loop","0", out_path], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def main():
    with sync_playwright() as p:
        browser = p.chromium.launch(executable_path=EDGE,
                                    args=["--no-sandbox","--disable-gpu","--force-color-profile=srgb"])
        for exp in EXPS:
            try:
                fd = os.path.join(FRM, exp["key"])
                os.makedirs(fd, exist_ok=True)
                for f in os.listdir(fd):
                    os.remove(os.path.join(fd, f))
                print("START", exp["key"])
                page = browser.new_page(viewport={"width":1000,"height":700}, device_scale_factor=1)
                page.goto("file://"+exp["file"], wait_until="load")
                page.wait_for_timeout(900)
                page.add_style_tag(content=HIDE_CSS)
                page.wait_for_timeout(250)
                canvas = page.query_selector("canvas")
                if exp["driver"] == "play":
                    page.evaluate("const b=document.getElementById('play'); if(b) b.click();")
                frames = max(2, int(exp["fps"]*exp["dur"]))
                reemit = exp.get("reemit")
                reemit_frames = int(exp["fps"]*reemit) if reemit else 0
                for i in range(frames):
                    if exp["driver"] == "slider":
                        val = exp["lo"] + triangle(i/frames)*(exp["hi"]-exp["lo"])
                        sid = exp["slider"]
                        page.evaluate("(v)=>{const el=document.getElementById(%r); if(el){el.value=v; el.dispatchEvent(new Event('input'));}}" % sid, val)
                    if reemit_frames and i>0 and i%reemit_frames==0:
                        page.evaluate("const b=document.getElementById('emit'); if(b) b.click();")
                    page.wait_for_timeout(int(1000/exp["fps"]))
                    canvas.screenshot(path=os.path.join(fd,"%03d.png"%(i+1)))
                    if (i+1)%15==0: print("  frame", i+1, "/", frames)
                page.close()
                out = os.path.join(OUT, exp["key"]+".gif")
                make_gif(fd, out, exp["fps"],
                         w=exp.get("w",360), h=exp.get("h",240),
                         colors=exp.get("colors",64), pad=exp.get("pad","white"))
                print("DONE %-12s %8.1f KB" % (exp["key"], os.path.getsize(out)/1024))
            except Exception as e:
                print("FAIL", exp["key"], repr(e))
        browser.close()
    print("ALL DONE ->", OUT)

if __name__ == "__main__":
    main()
