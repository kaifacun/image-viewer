#!/usr/bin/env python3
"""문서 모드 시험 파일 → scratch/docs (git 제외): 텍스트·마크다운·코드·RTF·DOCX(textutil)·여러 쪽 PDF."""
import os, subprocess
OUT = os.path.join(os.path.dirname(__file__), "..", "scratch", "docs")
os.makedirs(OUT, exist_ok=True)
w = lambda n, s: open(os.path.join(OUT, n), "w").write(s)
w("notes.txt", "KaiIV 문서 모드 시험\n\n" + "\n".join(f"{i:03d} 줄 — 한글과 English 가 섞인 본문입니다." for i in range(1, 200)))
w("readme.md", "# KaiIV\n\n빠른 이미지 뷰어.\n\n## 기능\n- 사진\n- 비디오\n- 문서\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n")
w("main.rs", "//! 예시 코드\nfn main() {\n    let v: Vec<u32> = (0..10).collect();\n    println!(\"{:?}\", v);\n}\n" * 10)
w("letter.html", "<h1>안내문</h1><p>KaiIV 문서 모드 <b>시험</b> 문서입니다.</p>" + "<p>본문 단락입니다.</p>" * 30)
for fmt in ("rtf", "docx"):
    subprocess.run(["textutil", "-convert", fmt, "-output", os.path.join(OUT, f"letter.{fmt}"), os.path.join(OUT, "letter.html")], check=True)
os.remove(os.path.join(OUT, "letter.html"))

def pdf(name, pages):
    """pages: [(w, h, rotate, 내용 연산자)] — 쪽마다 쪽 번호 크기의 막대"""
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", None, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    kids = []
    for i, (pw, ph, rot) in enumerate(pages):
        content = f"0.2 0.4 0.9 rg 40 {ph-120} {pw-80} 60 re f BT /F1 48 Tf 0 0 0 rg 60 {ph/2} Td (Page {i+1}) Tj ET"
        objs.append(f"<< /Length {len(content)} >>\nstream\n{content}\nendstream")
        cid = len(objs)
        objs.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw} {ph}] /Rotate {rot} /Resources << /Font << /F1 3 0 R >> >> /Contents {cid} 0 R >>")
        kids.append(f"{len(objs)} 0 R")
    objs[1] = f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {len(kids)} >>"
    out = "%PDF-1.4\n"; offs = []
    for i, o in enumerate(objs):
        offs.append(len(out)); out += f"{i+1} 0 obj\n{o}\nendobj\n"
    x = len(out)
    out += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n" + "".join(f"{o:010} 00000 n \n" for o in offs)
    out += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n"
    open(os.path.join(OUT, name), "w").write(out)
pdf("report.pdf", [(595, 842, 0), (595, 842, 0), (842, 595, 0), (595, 842, 90), (595, 842, 0)])
print("\n".join(sorted(os.listdir(OUT))))
