#!/usr/bin/env python3
"""
High-design 16:9 PPTX renderer for Local Notebook Agent Studio.
Supports 5 curated editorial themes and 12 rich visual layouts:
title, section, agenda, bullets, two_column, cards, metrics,
timeline, table, quote, formula, and summary.

Usage: python3 scripts/generate_pptx.py <input_payload.json> <output.pptx>
"""
import json
import re
import sys
from pathlib import Path

from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE
from pptx.enum.text import MSO_ANCHOR, PP_ALIGN
from pptx.util import Inches, Pt


THEMES = {
    "executive_navy": {
        "dark_bg": RGBColor(0x0F, 0x17, 0x2A),
        "dark_surface": RGBColor(0x1E, 0x29, 0x3B),
        "light_bg": RGBColor(0xF8, 0xFA, 0xFC),
        "card_bg": RGBColor(0xFF, 0xFF, 0xFF),
        "card_alt": RGBColor(0xF1, 0xF5, 0xF9),
        "primary": RGBColor(0x1D, 0x4E, 0xD8),
        "accent": RGBColor(0x0E, 0xA5, 0xE9),
        "warm": RGBColor(0xD9, 0x77, 0x06),
        "border": RGBColor(0xE2, 0xE8, 0xF0),
        "text_dark": RGBColor(0x0F, 0x17, 0x2A),
        "text_body": RGBColor(0x33, 0x41, 0x55),
        "text_muted": RGBColor(0x64, 0x74, 0x8B),
        "text_on_dark": RGBColor(0xF8, 0xFA, 0xFC),
        "text_on_dark_sub": RGBColor(0xCB, 0xD5, 0xE1),
    },
    "editorial_slate": {
        "dark_bg": RGBColor(0x18, 0x18, 0x1B),
        "dark_surface": RGBColor(0x27, 0x27, 0x2A),
        "light_bg": RGBColor(0xFA, 0xFA, 0xF9),
        "card_bg": RGBColor(0xFF, 0xFF, 0xFF),
        "card_alt": RGBColor(0xF4, 0xF4, 0xF5),
        "primary": RGBColor(0x4F, 0x46, 0xE5),
        "accent": RGBColor(0x06, 0xB6, 0xD4),
        "warm": RGBColor(0xE1, 0x1D, 0x48),
        "border": RGBColor(0xE4, 0xE4, 0xE7),
        "text_dark": RGBColor(0x18, 0x18, 0x1B),
        "text_body": RGBColor(0x3F, 0x3F, 0x46),
        "text_muted": RGBColor(0x71, 0x71, 0x7A),
        "text_on_dark": RGBColor(0xFA, 0xFA, 0xFA),
        "text_on_dark_sub": RGBColor(0xD4, 0xD4, 0xD8),
    },
    "academic_sage": {
        "dark_bg": RGBColor(0x13, 0x2A, 0x22),
        "dark_surface": RGBColor(0x1E, 0x3B, 0x30),
        "light_bg": RGBColor(0xF7, 0xFA, 0xF8),
        "card_bg": RGBColor(0xFF, 0xFF, 0xFF),
        "card_alt": RGBColor(0xEC, 0xF4, 0xF0),
        "primary": RGBColor(0x0F, 0x76, 0x6E),
        "accent": RGBColor(0x05, 0x96, 0x69),
        "warm": RGBColor(0xB4, 0x53, 0x09),
        "border": RGBColor(0xD8, 0xE6, 0xDF),
        "text_dark": RGBColor(0x13, 0x2A, 0x22),
        "text_body": RGBColor(0x2D, 0x47, 0x3E),
        "text_muted": RGBColor(0x5C, 0x76, 0x6C),
        "text_on_dark": RGBColor(0xF2, 0xFA, 0xF6),
        "text_on_dark_sub": RGBColor(0xC2, 0xDC, 0xD1),
    },
    "warm_terracotta": {
        "dark_bg": RGBColor(0x29, 0x1C, 0x17),
        "dark_surface": RGBColor(0x3D, 0x2B, 0x24),
        "light_bg": RGBColor(0xFD, 0xFA, 0xF6),
        "card_bg": RGBColor(0xFF, 0xFF, 0xFF),
        "card_alt": RGBColor(0xF6, 0xEE, 0xE6),
        "primary": RGBColor(0xC2, 0x41, 0x0C),
        "accent": RGBColor(0xD9, 0x77, 0x06),
        "warm": RGBColor(0x9A, 0x34, 0x12),
        "border": RGBColor(0xE8, 0xDA, 0xCC),
        "text_dark": RGBColor(0x29, 0x1C, 0x17),
        "text_body": RGBColor(0x4A, 0x37, 0x2F),
        "text_muted": RGBColor(0x7C, 0x66, 0x5C),
        "text_on_dark": RGBColor(0xFF, 0xFB, 0xF5),
        "text_on_dark_sub": RGBColor(0xE5, 0xD3, 0xC6),
    },
    "midnight_aurora": {
        "dark_bg": RGBColor(0x0D, 0x11, 0x17),
        "dark_surface": RGBColor(0x16, 0x1B, 0x22),
        "light_bg": RGBColor(0xF5, 0xF7, 0xFB),
        "card_bg": RGBColor(0xFF, 0xFF, 0xFF),
        "card_alt": RGBColor(0xEB, 0xF0, 0xF7),
        "primary": RGBColor(0x25, 0x63, 0xEB),
        "accent": RGBColor(0x7C, 0x3A, 0xED),
        "warm": RGBColor(0x08, 0x91, 0xB2),
        "border": RGBColor(0xDC, 0xE3, 0xEE),
        "text_dark": RGBColor(0x11, 0x18, 0x27),
        "text_body": RGBColor(0x37, 0x41, 0x51),
        "text_muted": RGBColor(0x6B, 0x72, 0x80),
        "text_on_dark": RGBColor(0xF3, 0xF4, 0xF6),
        "text_on_dark_sub": RGBColor(0xC7, 0xD2, 0xFE),
    },
}

FONT_BODY = "Microsoft YaHei"
FONT_MONO = "Consolas"


def clean_md(text: str) -> str:
    s = str(text or "").strip()
    s = re.sub(r"\*\*(.+?)\*\*", r"\1", s)
    s = re.sub(r"`(.+?)`", r"\1", s)
    return s


def set_bg(slide, color: RGBColor) -> None:
    fill = slide.background.fill
    fill.solid()
    fill.fore_color.rgb = color


def add_rect(slide, left, top, width, height, fill_color, border_color=None, rounded=False, border_pt=1.0):
    shape_type = MSO_SHAPE.ROUNDED_RECTANGLE if rounded else MSO_SHAPE.RECTANGLE
    sh = slide.shapes.add_shape(shape_type, left, top, width, height)
    if rounded:
        try:
            sh.adjustments[0] = 0.06
        except Exception:
            pass
    sh.fill.solid()
    sh.fill.fore_color.rgb = fill_color
    if border_color is not None:
        sh.line.color.rgb = border_color
        sh.line.width = Pt(border_pt)
    else:
        sh.line.fill.background()
    return sh


def add_text(
    slide,
    left,
    top,
    width,
    height,
    text,
    size=15,
    color=None,
    bold=False,
    italic=False,
    align=PP_ALIGN.LEFT,
    font_name=FONT_BODY,
    valign=MSO_ANCHOR.TOP,
):
    tb = slide.shapes.add_textbox(left, top, width, height)
    tf = tb.text_frame
    tf.word_wrap = True
    tf.vertical_anchor = valign
    tf.margin_left = Inches(0.04)
    tf.margin_right = Inches(0.04)
    tf.margin_top = Inches(0.02)
    tf.margin_bottom = Inches(0.02)
    p = tf.paragraphs[0]
    p.text = clean_md(text)
    p.alignment = align
    p.font.name = font_name
    p.font.size = Pt(size)
    p.font.bold = bold
    p.font.italic = italic
    if color is not None:
        p.font.color.rgb = color
    return tb


def add_header(slide, theme, title: str, subtitle: str, eyebrow: str = "") -> float:
    """Renders crisp editorial slide header and returns top Y (in inches) for body content."""
    # Top accent bar along full slide edge
    add_rect(slide, Inches(0), Inches(0), Inches(13.333), Inches(0.07), theme["primary"])

    y = 0.34
    if eyebrow:
        add_text(
            slide,
            Inches(0.65),
            Inches(y),
            Inches(11.8),
            Inches(0.28),
            eyebrow.upper(),
            size=10,
            color=theme["primary"],
            bold=True,
        )
        y += 0.25

    add_text(
        slide,
        Inches(0.65),
        Inches(y),
        Inches(12.0),
        Inches(0.58),
        title,
        size=25,
        color=theme["text_dark"],
        bold=True,
    )
    y += 0.56

    if subtitle:
        add_text(
            slide,
            Inches(0.65),
            Inches(y),
            Inches(12.0),
            Inches(0.38),
            subtitle,
            size=13,
            color=theme["text_muted"],
        )
        y += 0.38

    # Thin divider rule
    add_rect(slide, Inches(0.65), Inches(y + 0.04), Inches(12.03), Inches(0.02), theme["border"])
    return y + 0.22


def add_callout_bar(slide, theme, callout: str) -> None:
    if not callout.strip():
        return
    bar_y = Inches(6.22)
    add_rect(slide, Inches(0.65), bar_y, Inches(12.03), Inches(0.58), theme["card_alt"], theme["border"], rounded=True)
    add_rect(slide, Inches(0.65), bar_y, Inches(0.1), Inches(0.58), theme["primary"])
    add_text(
        slide,
        Inches(0.88),
        bar_y + Inches(0.08),
        Inches(11.65),
        Inches(0.42),
        callout,
        size=12.5,
        color=theme["text_dark"],
        bold=True,
        valign=MSO_ANCHOR.MIDDLE,
    )


def add_footer(slide, theme, deck_title: str, page_no: int, total: int, dark: bool = False) -> None:
    col = theme["text_on_dark_sub"] if dark else theme["text_muted"]
    add_text(
        slide,
        Inches(0.65),
        Inches(7.02),
        Inches(9.5),
        Inches(0.3),
        deck_title,
        size=9.5,
        color=col,
    )
    add_text(
        slide,
        Inches(10.5),
        Inches(7.02),
        Inches(2.18),
        Inches(0.3),
        f"{page_no} / {total}",
        size=9.5,
        color=col,
        bold=True,
        align=PP_ALIGN.RIGHT,
    )


def render_cover_or_summary(slide, theme, layout: str, title: str, subtitle: str, bullets: list, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["dark_bg"])
    # Decorative geometric accent column on right
    add_rect(slide, Inches(10.6), Inches(0), Inches(2.733), Inches(7.5), theme["dark_surface"])
    add_rect(slide, Inches(10.6), Inches(0), Inches(0.08), Inches(7.5), theme["accent"])
    add_rect(slide, Inches(0.85), Inches(1.75), Inches(1.2), Inches(0.09), theme["accent"])

    kicker = "KEY TAKEAWAYS & SUMMARY" if layout == "summary" else "PRESENTATION"
    add_text(slide, Inches(0.85), Inches(1.38), Inches(8.8), Inches(0.32), kicker, size=11, color=theme["accent"], bold=True)
    add_text(slide, Inches(0.85), Inches(2.05), Inches(9.3), Inches(1.7), title, size=38 if layout == "title" else 34, color=theme["text_on_dark"], bold=True)

    sub_y = 3.85
    if subtitle:
        add_text(slide, Inches(0.85), Inches(sub_y), Inches(9.1), Inches(0.75), subtitle, size=17, color=theme["text_on_dark_sub"])
        sub_y += 0.85

    if bullets:
        box = slide.shapes.add_textbox(Inches(0.85), Inches(sub_y), Inches(9.1), Inches(2.2))
        tf = box.text_frame
        tf.word_wrap = True
        for i, b in enumerate(bullets[:5]):
            p = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
            p.text = f"▸  {clean_md(b)}"
            p.font.name = FONT_BODY
            p.font.size = Pt(15.5)
            p.font.color.rgb = theme["text_on_dark_sub"]
            p.space_after = Pt(10)

    add_footer(slide, theme, deck_title, idx, total, dark=True)


def render_section(slide, theme, title: str, subtitle: str, section_no: str, bullets: list, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["dark_bg"])
    add_rect(slide, Inches(0), Inches(0), Inches(0.28), Inches(7.5), theme["primary"])
    num = section_no or f"{idx:02d}"
    add_text(slide, Inches(0.95), Inches(1.65), Inches(2.2), Inches(1.1), num, size=60, color=theme["accent"], bold=True)
    add_rect(slide, Inches(0.95), Inches(2.85), Inches(1.6), Inches(0.07), theme["primary"])
    add_text(slide, Inches(0.95), Inches(3.1), Inches(11.2), Inches(1.3), title, size=34, color=theme["text_on_dark"], bold=True)
    sub_text = subtitle or (bullets[0] if bullets else "")
    if sub_text:
        add_text(slide, Inches(0.95), Inches(4.5), Inches(10.5), Inches(1.2), sub_text, size=17, color=theme["text_on_dark_sub"])
    add_footer(slide, theme, deck_title, idx, total, dark=True)


def render_agenda(slide, theme, title: str, subtitle: str, bullets: list, steps: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle, eyebrow="Agenda")
    items = []
    if steps:
        for i, st in enumerate(steps[:6], start=1):
            items.append((str(st.get("step") or f"{i:02d}"), str(st.get("title") or ""), str(st.get("detail") or "")))
    else:
        for i, b in enumerate(bullets[:6], start=1):
            parts = clean_md(b).split("：", 1) if "：" in b else clean_md(b).split(":", 1)
            t = parts[0].strip()
            d = parts[1].strip() if len(parts) > 1 else ""
            items.append((f"{i:02d}", t, d))

    cols = 2 if len(items) > 3 else 1
    avail_h = 5.95 - top_y - (0.7 if callout else 0.0)
    rows = (len(items) + cols - 1) // cols if items else 1
    row_h = min(1.35, (avail_h - (rows - 1) * 0.2) / max(rows, 1))
    col_w = 5.85 if cols == 2 else 12.0

    for i, (num, heading, detail) in enumerate(items):
        r = i % rows if cols == 2 else i
        c = i // rows if cols == 2 else 0
        x = 0.65 + c * (col_w + 0.33)
        y = top_y + r * (row_h + 0.2)
        add_rect(slide, Inches(x), Inches(y), Inches(col_w), Inches(row_h), theme["card_bg"], theme["border"], rounded=True)
        # Number badge
        badge = add_rect(slide, Inches(x + 0.18), Inches(y + 0.22), Inches(0.58), Inches(0.58), theme["primary"], rounded=True)
        add_text(slide, Inches(x + 0.18), Inches(y + 0.22), Inches(0.58), Inches(0.58), num, size=16, color=theme["text_on_dark"], bold=True, align=PP_ALIGN.CENTER, valign=MSO_ANCHOR.MIDDLE)
        add_text(slide, Inches(x + 0.92), Inches(y + 0.16), Inches(col_w - 1.1), Inches(0.4), heading, size=15.5, color=theme["text_dark"], bold=True)
        if detail:
            add_text(slide, Inches(x + 0.92), Inches(y + 0.56), Inches(col_w - 1.1), Inches(row_h - 0.64), detail, size=12.5, color=theme["text_body"])

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_cards(slide, theme, title: str, subtitle: str, cards: list, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle)
    card_list = []
    if cards:
        for c in cards[:6]:
            card_list.append((str(c.get("tag") or ""), str(c.get("title") or ""), str(c.get("body") or "")))
    else:
        for b in bullets[:6]:
            parts = clean_md(b).split("：", 1) if "：" in b else clean_md(b).split(":", 1)
            card_list.append(("", parts[0].strip(), parts[1].strip() if len(parts) > 1 else ""))

    n = max(len(card_list), 1)
    cols = 3 if n in (3, 5, 6) else 2
    rows = (n + cols - 1) // cols
    gap_x, gap_y = 0.28, 0.26
    total_w = 12.03
    avail_h = 6.02 - top_y - (0.72 if callout else 0.0)
    card_w = (total_w - gap_x * (cols - 1)) / cols
    card_h = (avail_h - gap_y * (rows - 1)) / rows
    stripe_colors = [theme["primary"], theme["accent"], theme["warm"]]

    for i, (tag, ct, cb) in enumerate(card_list):
        r, c = divmod(i, cols)
        x = 0.65 + c * (card_w + gap_x)
        y = top_y + r * (card_h + gap_y)
        accent = stripe_colors[i % len(stripe_colors)]
        add_rect(slide, Inches(x), Inches(y), Inches(card_w), Inches(card_h), theme["card_bg"], theme["border"], rounded=True)
        add_rect(slide, Inches(x), Inches(y), Inches(card_w), Inches(0.08), accent)
        ty = y + 0.18
        if tag:
            add_text(slide, Inches(x + 0.2), Inches(ty), Inches(card_w - 0.4), Inches(0.28), tag, size=10, color=accent, bold=True)
            ty += 0.28
        add_text(slide, Inches(x + 0.2), Inches(ty), Inches(card_w - 0.4), Inches(0.45), ct, size=15.5, color=theme["text_dark"], bold=True)
        add_text(slide, Inches(x + 0.2), Inches(ty + 0.48), Inches(card_w - 0.4), Inches(max(0.5, y + card_h - ty - 0.6)), cb, size=13, color=theme["text_body"])

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_metrics(slide, theme, title: str, subtitle: str, metrics: list, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle, eyebrow="Key Metrics")
    m_list = metrics[:4] if metrics else []
    if not m_list:
        for b in bullets[:3]:
            parts = clean_md(b).split("：", 1) if "：" in b else clean_md(b).split(":", 1)
            m_list.append({"value": parts[0][:14], "label": parts[1] if len(parts) > 1 else parts[0], "detail": ""})

    n = max(len(m_list), 1)
    gap = 0.28
    card_w = (12.03 - gap * (n - 1)) / n
    card_h = 2.35 if bullets else 3.4
    colors = [theme["primary"], theme["accent"], theme["warm"], theme["primary"]]

    for i, m in enumerate(m_list):
        x = 0.65 + i * (card_w + gap)
        add_rect(slide, Inches(x), Inches(top_y), Inches(card_w), Inches(card_h), theme["card_bg"], theme["border"], rounded=True)
        add_rect(slide, Inches(x), Inches(top_y), Inches(card_w), Inches(0.08), colors[i % len(colors)])
        add_text(slide, Inches(x + 0.2), Inches(top_y + 0.28), Inches(card_w - 0.4), Inches(0.85), str(m.get("value") or ""), size=34, color=colors[i % len(colors)], bold=True, align=PP_ALIGN.CENTER)
        add_text(slide, Inches(x + 0.2), Inches(top_y + 1.18), Inches(card_w - 0.4), Inches(0.45), str(m.get("label") or ""), size=14, color=theme["text_dark"], bold=True, align=PP_ALIGN.CENTER)
        det = str(m.get("detail") or "")
        if det:
            add_text(slide, Inches(x + 0.2), Inches(top_y + 1.65), Inches(card_w - 0.4), Inches(card_h - 1.78), det, size=11.5, color=theme["text_muted"], align=PP_ALIGN.CENTER)

    if bullets:
        by = top_y + card_h + 0.28
        bh = max(1.2, 6.0 - by - (0.72 if callout else 0.0))
        add_rect(slide, Inches(0.65), Inches(by), Inches(12.03), Inches(bh), theme["card_bg"], theme["border"], rounded=True)
        tb = slide.shapes.add_textbox(Inches(0.9), Inches(by + 0.16), Inches(11.5), Inches(bh - 0.28))
        tf = tb.text_frame
        tf.word_wrap = True
        for i, b in enumerate(bullets[:4]):
            p = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
            p.text = f"•  {clean_md(b)}"
            p.font.name = FONT_BODY
            p.font.size = Pt(13.5)
            p.font.color.rgb = theme["text_body"]
            p.space_after = Pt(7)

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_timeline(slide, theme, title: str, subtitle: str, steps: list, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle, eyebrow="Process & Timeline")
    s_list = steps[:5] if steps else []
    if not s_list:
        for i, b in enumerate(bullets[:5], start=1):
            parts = clean_md(b).split("：", 1) if "：" in b else clean_md(b).split(":", 1)
            s_list.append({"step": f"0{i}", "title": parts[0], "detail": parts[1] if len(parts) > 1 else ""})

    n = max(len(s_list), 1)
    gap = 0.22
    col_w = (12.03 - gap * (n - 1)) / n
    line_y = top_y + 0.55
    # Connecting horizontal rail
    add_rect(slide, Inches(0.65 + col_w / 2), Inches(line_y), Inches(12.03 - col_w), Inches(0.05), theme["border"])

    card_top = top_y + 1.15
    card_h = max(2.4, 6.0 - card_top - (0.72 if callout else 0.0))

    for i, st in enumerate(s_list):
        x = 0.65 + i * (col_w + gap)
        cx = x + col_w / 2 - 0.34
        circle = slide.shapes.add_shape(MSO_SHAPE.OVAL, Inches(cx), Inches(top_y + 0.22), Inches(0.68), Inches(0.68))
        circle.fill.solid()
        circle.fill.fore_color.rgb = theme["primary"] if i % 2 == 0 else theme["accent"]
        circle.line.fill.background()
        lbl = str(st.get("step") or f"{i + 1:02d}")
        add_text(slide, Inches(cx), Inches(top_y + 0.22), Inches(0.68), Inches(0.68), lbl, size=14, color=theme["text_on_dark"], bold=True, align=PP_ALIGN.CENTER, valign=MSO_ANCHOR.MIDDLE)

        add_rect(slide, Inches(x), Inches(card_top), Inches(col_w), Inches(card_h), theme["card_bg"], theme["border"], rounded=True)
        add_text(slide, Inches(x + 0.16), Inches(card_top + 0.18), Inches(col_w - 0.32), Inches(0.55), str(st.get("title") or ""), size=14.5, color=theme["text_dark"], bold=True, align=PP_ALIGN.CENTER)
        add_text(slide, Inches(x + 0.16), Inches(card_top + 0.78), Inches(col_w - 0.32), Inches(card_h - 0.92), str(st.get("detail") or ""), size=12.5, color=theme["text_body"])

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_table(slide, theme, title: str, subtitle: str, table_obj: dict, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle, eyebrow="Comparison")
    headers = [clean_md(h) for h in (table_obj.get("headers") or [])]
    rows = [[clean_md(c) for c in r] for r in (table_obj.get("rows") or [])]
    if not headers or not rows:
        render_bullets(slide, theme, title, subtitle, bullets, callout, deck_title, idx, total)
        return

    ncols = len(headers)
    nrows = min(len(rows), 7) + 1
    avail_h = min(4.35, 6.0 - top_y - (0.72 if callout else 0.0))
    tbl_shape = slide.shapes.add_table(nrows, ncols, Inches(0.65), Inches(top_y + 0.06), Inches(12.03), Inches(avail_h))
    tbl = tbl_shape.table

    for ci, h in enumerate(headers):
        cell = tbl.cell(0, ci)
        cell.fill.solid()
        cell.fill.fore_color.rgb = theme["primary"]
        cell.vertical_anchor = MSO_ANCHOR.MIDDLE
        p = cell.text_frame.paragraphs[0]
        p.text = h
        p.font.name = FONT_BODY
        p.font.size = Pt(13)
        p.font.bold = True
        p.font.color.rgb = theme["text_on_dark"]

    for ri, row in enumerate(rows[: nrows - 1], start=1):
        bg = theme["card_bg"] if ri % 2 == 1 else theme["card_alt"]
        for ci in range(ncols):
            val = row[ci] if ci < len(row) else ""
            cell = tbl.cell(ri, ci)
            cell.fill.solid()
            cell.fill.fore_color.rgb = bg
            cell.vertical_anchor = MSO_ANCHOR.MIDDLE
            p = cell.text_frame.paragraphs[0]
            p.text = val
            p.font.name = FONT_BODY
            p.font.size = Pt(12.5)
            p.font.bold = ci == 0
            p.font.color.rgb = theme["text_dark"] if ci == 0 else theme["text_body"]

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_two_column(slide, theme, title: str, subtitle: str, columns: list, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle)
    col_data = []
    if columns and len(columns) >= 2:
        for c in columns[:2]:
            col_data.append((str(c.get("heading") or ""), [clean_md(x) for x in (c.get("items") or [])]))
    else:
        mid = (len(bullets) + 1) // 2
        col_data = [("", [clean_md(x) for x in bullets[:mid]]), ("", [clean_md(x) for x in bullets[mid:]])]

    avail_h = 6.0 - top_y - (0.72 if callout else 0.0)
    col_w = 5.85
    accents = [theme["primary"], theme["accent"]]

    for ci, (heading, items) in enumerate(col_data):
        x = 0.65 + ci * (col_w + 0.33)
        add_rect(slide, Inches(x), Inches(top_y), Inches(col_w), Inches(avail_h), theme["card_bg"], theme["border"], rounded=True)
        add_rect(slide, Inches(x), Inches(top_y), Inches(col_w), Inches(0.09), accents[ci % 2])
        body_y = top_y + 0.22
        if heading:
            add_text(slide, Inches(x + 0.25), Inches(body_y), Inches(col_w - 0.5), Inches(0.42), heading, size=16.5, color=theme["text_dark"], bold=True)
            body_y += 0.52
        tb = slide.shapes.add_textbox(Inches(x + 0.25), Inches(body_y), Inches(col_w - 0.5), Inches(top_y + avail_h - body_y - 0.16))
        tf = tb.text_frame
        tf.word_wrap = True
        for bi, item in enumerate(items[:6]):
            p = tf.paragraphs[0] if bi == 0 else tf.add_paragraph()
            p.text = f"▸  {item}"
            p.font.name = FONT_BODY
            p.font.size = Pt(14)
            p.font.color.rgb = theme["text_body"]
            p.space_after = Pt(10)

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_quote_or_formula(slide, theme, layout: str, title: str, subtitle: str, formula: str, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle, eyebrow="Core Formula" if layout == "formula" else "Key Insight")
    hero = clean_md(formula) if (layout == "formula" and formula.strip()) else (clean_md(bullets[0]) if bullets else "")
    rest = bullets if (layout == "formula" and formula.strip()) else bullets[1:]

    card_h = 2.15 if rest else 3.8
    add_rect(slide, Inches(0.85), Inches(top_y + 0.1), Inches(11.63), Inches(card_h), theme["dark_bg"], theme["primary"], rounded=True, border_pt=1.5)
    add_rect(slide, Inches(0.85), Inches(top_y + 0.1), Inches(0.14), Inches(card_h), theme["accent"])
    hero_txt = f"“{hero}”" if layout == "quote" else hero
    add_text(
        slide,
        Inches(1.25),
        Inches(top_y + 0.25),
        Inches(10.9),
        Inches(card_h - 0.3),
        hero_txt,
        size=22 if layout == "quote" else 21,
        color=theme["text_on_dark"],
        bold=layout == "formula",
        italic=layout == "quote",
        align=PP_ALIGN.CENTER,
        font_name=FONT_MONO if layout == "formula" else FONT_BODY,
        valign=MSO_ANCHOR.MIDDLE,
    )

    if rest:
        ry = top_y + card_h + 0.32
        rh = max(1.2, 6.0 - ry - (0.72 if callout else 0.0))
        add_rect(slide, Inches(0.85), Inches(ry), Inches(11.63), Inches(rh), theme["card_bg"], theme["border"], rounded=True)
        tb = slide.shapes.add_textbox(Inches(1.15), Inches(ry + 0.18), Inches(11.0), Inches(rh - 0.3))
        tf = tb.text_frame
        tf.word_wrap = True
        for bi, b in enumerate(rest[:5]):
            p = tf.paragraphs[0] if bi == 0 else tf.add_paragraph()
            p.text = f"•  {clean_md(b)}"
            p.font.name = FONT_BODY
            p.font.size = Pt(14.5)
            p.font.color.rgb = theme["text_body"]
            p.space_after = Pt(8)

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_bullets(slide, theme, title: str, subtitle: str, bullets: list, callout: str, deck_title: str, idx: int, total: int):
    set_bg(slide, theme["light_bg"])
    top_y = add_header(slide, theme, title, subtitle)
    avail_h = 6.0 - top_y - (0.72 if callout else 0.0)
    items = [clean_md(b) for b in bullets[:6]]
    if not items:
        add_footer(slide, theme, deck_title, idx, total)
        return

    gap = 0.16
    row_h = min(1.02, (avail_h - gap * (len(items) - 1)) / len(items))
    for i, item in enumerate(items):
        y = top_y + i * (row_h + gap)
        add_rect(slide, Inches(0.65), Inches(y), Inches(12.03), Inches(row_h), theme["card_bg"], theme["border"], rounded=True)
        add_rect(slide, Inches(0.65), Inches(y), Inches(0.09), Inches(row_h), theme["primary"] if i % 2 == 0 else theme["accent"])
        add_text(
            slide,
            Inches(0.92),
            Inches(y + 0.08),
            Inches(11.55),
            Inches(row_h - 0.16),
            item,
            size=14.5,
            color=theme["text_body"],
            valign=MSO_ANCHOR.MIDDLE,
        )

    add_callout_bar(slide, theme, callout)
    add_footer(slide, theme, deck_title, idx, total)


def render_pptx(payload: dict, out_path: Path) -> None:
    prs = Presentation()
    prs.slide_width = Inches(13.333)
    prs.slide_height = Inches(7.5)
    blank = prs.slide_layouts[6]

    theme_name = str(payload.get("theme") or "executive_navy").lower()
    theme = THEMES.get(theme_name, THEMES["executive_navy"])
    deck_title = str(payload.get("title") or "演示文稿")
    deck_subtitle = str(payload.get("subtitle") or "")
    slides = payload.get("slides") or []
    if not slides:
        slides = [{"layout": "title", "title": deck_title, "subtitle": deck_subtitle, "bullets": [], "speaker_notes": ""}]

    total = len(slides)
    for idx, s in enumerate(slides, start=1):
        slide = prs.slides.add_slide(blank)
        layout = str(s.get("layout") or "bullets").lower()
        title = str(s.get("title") or f"第 {idx} 页")
        subtitle = str(s.get("subtitle") or (deck_subtitle if idx == 1 else ""))
        section_no = str(s.get("section_number") or "")
        bullets = [str(b) for b in (s.get("bullets") or []) if str(b).strip()]
        columns = s.get("columns") or []
        cards = s.get("cards") or []
        metrics = s.get("metrics") or []
        steps = s.get("steps") or []
        table_obj = s.get("table") or {}
        formula = str(s.get("formula") or "")
        callout = str(s.get("callout") or "")
        notes_text = str(s.get("speaker_notes") or "").strip()

        if layout in ("title", "summary"):
            render_cover_or_summary(slide, theme, layout, title, subtitle, bullets, deck_title, idx, total)
        elif layout == "section":
            render_section(slide, theme, title, subtitle, section_no, bullets, deck_title, idx, total)
        elif layout == "agenda":
            render_agenda(slide, theme, title, subtitle, bullets, steps, callout, deck_title, idx, total)
        elif layout == "cards":
            render_cards(slide, theme, title, subtitle, cards, bullets, callout, deck_title, idx, total)
        elif layout == "metrics":
            render_metrics(slide, theme, title, subtitle, metrics, bullets, callout, deck_title, idx, total)
        elif layout == "timeline":
            render_timeline(slide, theme, title, subtitle, steps, bullets, callout, deck_title, idx, total)
        elif layout == "table":
            render_table(slide, theme, title, subtitle, table_obj, bullets, callout, deck_title, idx, total)
        elif layout == "two_column":
            render_two_column(slide, theme, title, subtitle, columns, bullets, callout, deck_title, idx, total)
        elif layout in ("quote", "formula"):
            render_quote_or_formula(slide, theme, layout, title, subtitle, formula, bullets, callout, deck_title, idx, total)
        else:
            render_bullets(slide, theme, title, subtitle, bullets, callout, deck_title, idx, total)

        if notes_text:
            slide.notes_slide.notes_text_frame.text = notes_text

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
