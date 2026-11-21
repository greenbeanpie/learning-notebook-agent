#!/usr/bin/env python3
"""
Local PPTX generator tool for Local Notebook Agent Studio (Requirement 5).
Usage: python3 scripts/generate_pptx.py <input_payload.json> <output.pptx>
"""
import json
import sys
from pathlib import Path

from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE
from pptx.enum.text import PP_ALIGN
from pptx.util import Inches, Pt

BG_DARK = RGBColor(0x13, 0x13, 0x14)
CARD_BG = RGBColor(0x1E, 0x1F, 0x20)
ACCENT_BLUE = RGBColor(0xA8, 0xC7, 0xFA)
ACCENT_GOLD = RGBColor(0xFD, 0xD6, 0x63)
TEXT_PRIMARY = RGBColor(0xE3, 0xE3, 0xE3)
TEXT_SECONDARY = RGBColor(0xC4, 0xC7, 0xC5)
TEXT_MUTED = RGBColor(0x8E, 0x91, 0x8F)


def set_slide_bg(slide, color):
    fill = slide.background.fill
    fill.solid()
    fill.fore_color.rgb = color


def add_footer(slide, deck_title, page_no, total):
    box = slide.shapes.add_textbox(Inches(0.6), Inches(6.95), Inches(12.1), Inches(0.35))
    tf = box.text_frame
    tf.word_wrap = True
    p = tf.paragraphs[0]
    p.text = f"{deck_title}   |   {page_no} / {total}"
    p.font.size = Pt(10)
    p.font.color.rgb = TEXT_MUTED
    p.alignment = PP_ALIGN.RIGHT


def render_pptx(payload: dict, out_path: Path) -> None:
    prs = Presentation()
    prs.slide_width = Inches(13.333)
    prs.slide_height = Inches(7.5)
    blank = prs.slide_layouts[6]

    deck_title = str(payload.get("title") or "演示文稿")
    slides = payload.get("slides") or []
    if not slides:
        slides = [{"layout": "title", "title": deck_title, "bullets": [], "speaker_notes": ""}]

    total = len(slides)
    for idx, s in enumerate(slides, start=1):
        slide = prs.slides.add_slide(blank)
        set_slide_bg(slide, BG_DARK)
        layout = str(s.get("layout") or "bullets").lower()
        title = str(s.get("title") or f"第 {idx} 页")
        bullets = [str(b) for b in (s.get("bullets") or []) if str(b).strip()]
        notes_text = str(s.get("speaker_notes") or "").strip()

        if layout in ("title", "section"):
            # Top accent bar
            bar = slide.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(0.9), Inches(2.2), Inches(1.4), Inches(0.08))
            bar.fill.solid()
            bar.fill.fore_color.rgb = ACCENT_BLUE if layout == "title" else ACCENT_GOLD
            bar.line.fill.background()

            tb = slide.shapes.add_textbox(Inches(0.9), Inches(2.45), Inches(11.5), Inches(1.8))
            tf = tb.text_frame
            tf.word_wrap = True
            p = tf.paragraphs[0]
            p.text = title
            p.font.size = Pt(40 if layout == "title" else 34)
            p.font.bold = True
            p.font.color.rgb = TEXT_PRIMARY

            if bullets:
                sub = slide.shapes.add_textbox(Inches(0.9), Inches(4.3), Inches(11.0), Inches(1.8))
                stf = sub.text_frame
                stf.word_wrap = True
                for i, b in enumerate(bullets[:4]):
                    sp = stf.paragraphs[0] if i == 0 else stf.add_paragraph()
                    sp.text = b
                    sp.font.size = Pt(18)
                    sp.font.color.rgb = TEXT_SECONDARY
                    sp.space_after = Pt(8)
        elif layout == "two_column" and len(bullets) >= 2:
            tb = slide.shapes.add_textbox(Inches(0.75), Inches(0.55), Inches(11.8), Inches(0.9))
            tf = tb.text_frame
            tf.word_wrap = True
            p = tf.paragraphs[0]
            p.text = title
            p.font.size = Pt(28)
            p.font.bold = True
            p.font.color.rgb = TEXT_PRIMARY

            mid = (len(bullets) + 1) // 2
            cols = [bullets[:mid], bullets[mid:]]
            for ci, col_items in enumerate(cols):
                left = Inches(0.75 + ci * 6.05)
                card = slide.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, left, Inches(1.65), Inches(5.75), Inches(4.9))
                card.fill.solid()
                card.fill.fore_color.rgb = CARD_BG
                card.line.color.rgb = RGBColor(0x3C, 0x40, 0x43)
                ctf = card.text_frame
                ctf.word_wrap = True
                ctf.margin_left = Inches(0.35)
                ctf.margin_right = Inches(0.35)
                ctf.margin_top = Inches(0.35)
                for bi, b in enumerate(col_items):
                    bp = ctf.paragraphs[0] if bi == 0 else ctf.add_paragraph()
                    bp.text = f"- {b}"
                    bp.font.size = Pt(16)
                    bp.font.color.rgb = TEXT_SECONDARY
                    bp.space_after = Pt(12)
        elif layout in ("quote", "formula"):
            tb = slide.shapes.add_textbox(Inches(0.75), Inches(0.55), Inches(11.8), Inches(0.9))
            tf = tb.text_frame
            tf.word_wrap = True
            p = tf.paragraphs[0]
            p.text = title
            p.font.size = Pt(28)
            p.font.bold = True
            p.font.color.rgb = TEXT_PRIMARY

            card = slide.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(1.0), Inches(1.85), Inches(11.3), Inches(4.3))
            card.fill.solid()
            card.fill.fore_color.rgb = CARD_BG
            card.line.color.rgb = ACCENT_BLUE
            ctf = card.text_frame
            ctf.word_wrap = True
            ctf.margin_left = Inches(0.5)
            ctf.margin_right = Inches(0.5)
            ctf.margin_top = Inches(0.5)
            for bi, b in enumerate(bullets):
                bp = ctf.paragraphs[0] if bi == 0 else ctf.add_paragraph()
                bp.text = f'"{b}"' if layout == "quote" else b
                bp.font.size = Pt(22 if bi == 0 else 16)
                bp.font.italic = layout == "quote"
                bp.font.color.rgb = ACCENT_BLUE if bi == 0 else TEXT_SECONDARY
                bp.alignment = PP_ALIGN.CENTER
                bp.space_after = Pt(14)
        else:
            tb = slide.shapes.add_textbox(Inches(0.75), Inches(0.55), Inches(11.8), Inches(0.9))
            tf = tb.text_frame
            tf.word_wrap = True
            p = tf.paragraphs[0]
            p.text = title
            p.font.size = Pt(28)
            p.font.bold = True
            p.font.color.rgb = TEXT_PRIMARY

            card = slide.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(0.75), Inches(1.65), Inches(11.8), Inches(4.95))
            card.fill.solid()
            card.fill.fore_color.rgb = CARD_BG
            card.line.color.rgb = RGBColor(0x3C, 0x40, 0x43)
            ctf = card.text_frame
            ctf.word_wrap = True
            ctf.margin_left = Inches(0.45)
            ctf.margin_right = Inches(0.45)
            ctf.margin_top = Inches(0.35)
            for bi, b in enumerate(bullets):
                bp = ctf.paragraphs[0] if bi == 0 else ctf.add_paragraph()
                bp.text = f"- {b}"
                bp.font.size = Pt(17)
                bp.font.color.rgb = TEXT_SECONDARY
                bp.space_after = Pt(12)

        if notes_text:
            slide.notes_slide.notes_text_frame.text = notes_text
        add_footer(slide, deck_title, idx, total)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    prs.save(str(out_path))


def main() -> int:
    if len(sys.argv) < 3:
        print("Usage: generate_pptx.py <input.json> <output.pptx>", file=sys.stderr)
        return 2
    in_path = Path(sys.argv[1])
    out_path = Path(sys.argv[2])
    payload = json.loads(in_path.read_text(encoding="utf-8"))
    render_pptx(payload, out_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
