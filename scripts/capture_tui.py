import sys
import os
from PIL import Image, ImageDraw, ImageFont

def render_ansi_or_text_to_png(txt_path, out_png_path):
    with open(txt_path, 'r', encoding='utf-8', errors='replace') as f:
        lines = f.read().splitlines()

import sys
import os
import unicodedata
from PIL import Image, ImageDraw, ImageFont

def get_char_display_width(ch):
    # East Asian Fullwidth / Wide characters take 2 columns
    ea = unicodedata.east_asian_width(ch)
    if ea in ('F', 'W'):
        return 2
    return 1

def render_ansi_or_text_to_png(txt_path, out_png_path):
    with open(txt_path, 'r', encoding='utf-8', errors='replace') as f:
        lines = f.read().splitlines()

    font_size = 16
    line_height = 22
    col_width = 9.5

    img_width = 110 * int(col_width * 2) // 2 + 50
    img_height = len(lines) * line_height + 40

    img = Image.new("RGB", (img_width, img_height), color=(18, 22, 28))
    draw = ImageDraw.Draw(img)

    font = None
    for c in [
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ]:
        if os.path.exists(c):
            try:
                font = ImageFont.truetype(c, font_size)
                break
            except Exception:
                pass
    if font is None:
        font = ImageFont.load_default()

    y = 20
    for line in lines:
        col = 0
        for ch in line:
            x = 20 + int(col * col_width)
            # Render character
            draw.text((x, y), ch, font=font, fill=(215, 225, 235))
            col += get_char_display_width(ch)
        y += line_height

    os.makedirs(os.path.dirname(os.path.abspath(out_png_path)), exist_ok=True)
    img.save(out_png_path)
    print(f"Captured PNG saved to: {out_png_path}")

    os.makedirs(os.path.dirname(os.path.abspath(out_png_path)), exist_ok=True)
    img.save(out_png_path)
    print(f"Captured PNG saved to: {out_png_path}")

if __name__ == '__main__':
    if len(sys.argv) < 3:
        print("Usage: python3 capture_to_png.py <input.txt> <output.png>")
        sys.exit(1)
    render_ansi_or_text_to_png(sys.argv[1], sys.argv[2])
