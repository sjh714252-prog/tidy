"""Synthetic desktop corpus mirroring the PR #2 Windows mix (118 files)."""
import os, random, shutil, sys, zipfile
from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
import openpyxl, docx

out = sys.argv[1]; fixtures = sys.argv[2]
random.seed(42)
os.makedirs(out, exist_ok=True)
pdfmetrics.registerFont(TTFont("Nanum", "/usr/share/fonts/truetype/nanum/NanumGothic.ttf"))
WORDS = "과금 내역서 회의록 보안점검 수탁자 보고서 제안서 예산 계약 정산 인터넷 회선 IPTV 고객 KT RCS 체크리스트 담당자 일정 검토 승인 요청 결과 분석 매출 성장 분기".split()
def sent(n=12): return " ".join(random.choice(WORDS) for _ in range(n)) + "."

def pdf(path, pages):
    c = canvas.Canvas(path); c.setFont("Nanum", 10)
    for _ in range(pages):
        y = 800
        for _ in range(60):
            c.drawString(40, y, sent(10)); y -= 13
        c.showPage()
    c.save()
def xlsx(path, rows):
    wb = openpyxl.Workbook(); ws = wb.active; ws.title = "RCS 내역"
    for i in range(rows):
        ws.append([i, random.choice(WORDS), random.randint(1000, 99999), sent(4)])
    wb.save(path)
def docx_(path, paras):
    d = docx.Document()
    for _ in range(paras): d.add_paragraph(sent(30))
    d.save(path)

n = 0
def name(stem, ext):
    global n; n += 1; return os.path.join(out, f"{stem} {n:03d}.{ext}")
for _ in range(20): pdf(name("보고서", "pdf"), random.randint(3, 30))
for _ in range(20): xlsx(name("과금 내역서", "xlsx"), random.randint(200, 3000))
for _ in range(15): docx_(name("제안서", "docx"), random.randint(50, 400))
for _ in range(6): shutil.copy(os.path.join(fixtures, "발표.pptx"), name("발표", "pptx"))
for _ in range(10):
    with open(name("메모", "txt"), "w", encoding="utf-8") as f: f.write("\n".join(sent() for _ in range(random.randint(20, 2000))))
for _ in range(5):
    with open(name("RCS 목록", "csv"), "w", encoding="utf-8") as f:
        f.write("\n".join(f"{i},{random.choice(WORDS)},{random.randint(1,9999)}" for i in range(random.randint(1000, 20000))))
for _ in range(5):
    with open(name("안내", "html"), "w", encoding="utf-8") as f: f.write("<html><body>" + "".join(f"<p>{sent()}</p>" for _ in range(300)) + "</body></html>")
# 36 unsupported: zip 23, lnk 9, hwp 2, exe 1
for _ in range(23):
    p = name("압축", "zip")
    with zipfile.ZipFile(p, "w") as z: z.writestr("data.bin", os.urandom(random.randint(100_000, 3_000_000)))
for _ in range(10):
    with open(name("바로가기", "lnk"), "wb") as f: f.write(os.urandom(1500))
for _ in range(2):
    with open(name("한글문서", "hwp"), "wb") as f: f.write(os.urandom(200_000))
with open(name("setup", "exe"), "wb") as f: f.write(os.urandom(5_000_000))
# 1 too big (>200MB): sparse
with open(name("영상", "mp4"), "wb") as f: f.truncate(210 * 1024 * 1024)
print(n, "files")
