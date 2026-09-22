#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
生成程序图标（纯标准库，不依赖 Pillow）：

  assets/icon.ico          通用 ICO
  src-tauri/icons/*.png    Tauri 打包需要的 PNG
  src-tauri/icons/icon.ico （同上，Tauri 也读这个）

画一个圆角方形（蓝紫渐变）+ 白色信号弧，4 倍超采样做抗锯齿。
小于 256 的尺寸用 BMP 格式塞进 ICO（兼容性最好），256 用 PNG。

用法： python tools/make_icon.py
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "assets" / "icon.ico"
TAURI_ICONS = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"

SUPER = 1024                              # 超采样画布边长
SIZES = [256, 128, 64, 48, 32, 16]

# Tauri 打包需要的 PNG（文件名不能改）
TAURI_PNGS = {32: "32x32.png", 128: "128x128.png", 256: "128x128@2x.png"}

# 渐变两端色
C1 = (79, 140, 255)                       # #4f8cff
C2 = (123, 92, 255)                       # #7b5cff


# --------------------------------------------------------------------------- #
# 绘制
# --------------------------------------------------------------------------- #

def _rounded_alpha(x: float, y: float, s: float, r: float) -> float:
    """点 (x,y) 是否落在边长为 s、圆角半径 r 的圆角方形内（返回 0/1）。"""
    qx = abs(x - s / 2) - (s / 2 - r)
    qy = abs(y - s / 2) - (s / 2 - r)
    if qx <= 0 or qy <= 0:
        return 1.0 if (qx <= 0 and qy <= 0) else 0.0
    return 1.0 if math.hypot(qx, qy) <= r else 0.0


def render_super() -> list[tuple[int, int, int, int]]:
    s = SUPER
    r = s * 0.22
    cx, cy = s / 2, s * 0.78               # 圆心（信号弧的发射点）
    radii = [0.155 * s, 0.295 * s, 0.435 * s]
    thick = 0.078 * s

    buf = []
    for y in range(s):
        for x in range(s):
            if _rounded_alpha(x + 0.5, y + 0.5, s, r) == 0.0:
                buf.append((0, 0, 0, 0))
                continue

            # 对角渐变底色
            t = (x + y) / (2 * s)
            bg = tuple(int(C1[i] + (C2[i] - C1[i]) * t) for i in range(3))

            dx = x + 0.5 - cx
            dy = y + 0.5 - cy
            dist = math.hypot(dx, dy)

            white = False
            # 底部圆点
            if dist <= 0.075 * s:
                white = True
            # 三段弧：只在垂直向上 ±45° 范围内
            elif dy < 0 and abs(dx) <= -dy * 1.0:
                for rr in radii:
                    if abs(dist - rr) <= thick / 2:
                        white = True
                        break

            buf.append((255, 255, 255, 255) if white else (*bg, 255))
    return buf


def downsample(src, src_size: int, dst_size: int) -> bytes:
    """盒式降采样，返回 RGBA bytes。"""
    k = src_size // dst_size
    out = bytearray()
    for y in range(dst_size):
        for x in range(dst_size):
            r = g = b = a = 0
            for yy in range(y * k, (y + 1) * k):
                base = yy * src_size
                for xx in range(x * k, (x + 1) * k):
                    p = src[base + xx]
                    r += p[0]; g += p[1]; b += p[2]; a += p[3]
            n = k * k
            # 先按 alpha 加权还原颜色，避免边缘发黑
            aa = a / n
            if a:
                rr, gg, bb = r / a, g / a, b / a
            else:
                rr = gg = bb = 0.0
            out += bytes((int(rr), int(gg), int(bb), int(aa)))
    return bytes(out)


# --------------------------------------------------------------------------- #
# 编码
# --------------------------------------------------------------------------- #

def _chunk(tag: bytes, data: bytes) -> bytes:
    return (struct.pack(">I", len(data)) + tag + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))


def png_bytes(size: int, rgba: bytes) -> bytes:
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)                                   # filter: None
        raw += rgba[y * stride:(y + 1) * stride]
    return (b"\x89PNG\r\n\x1a\n"
            + _chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + _chunk(b"IDAT", zlib.compress(bytes(raw), 9))
            + _chunk(b"IEND", b""))


def bmp_bytes(size: int, rgba: bytes) -> bytes:
    hdr = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = []
    for y in range(size - 1, -1, -1):                   # BMP 自下而上
        row = bytearray()
        for x in range(size):
            i = (y * size + x) * 4
            row += bytes((rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]))
        rows.append(bytes(row))
    mask_row = ((size + 31) // 32) * 4                  # 1bpp，行按 4 字节对齐
    return hdr + b"".join(rows) + b"\x00" * (mask_row * size)


def build_ico(items: list[tuple[int, bytes]]) -> bytes:
    n = len(items)
    out = struct.pack("<HHH", 0, 1, n)
    offset = 6 + 16 * n
    for size, blob in items:
        d = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", d, d, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    return out + b"".join(b for _, b in items)


# --------------------------------------------------------------------------- #

def main() -> int:
    print(f"渲染 {SUPER}x{SUPER} 超采样画布…")
    src = render_super()

    # 每个尺寸只降采样一次，PNG / BMP 共用
    rgba_cache = {}
    items = []
    for size in SIZES:
        rgba = downsample(src, SUPER, size)
        rgba_cache[size] = rgba
        blob = png_bytes(size, rgba) if size >= 256 else bmp_bytes(size, rgba)
        items.append((size, blob))
        print(f"  {size:>3}x{size:<3} {len(blob):>7} bytes "
              f"({'PNG' if size >= 256 else 'BMP'})")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(build_ico(items))
    print(f"\n已生成 {OUT}  ({OUT.stat().st_size} bytes)")

    # ---- Tauri 打包用的一套 ----
    TAURI_ICONS.mkdir(parents=True, exist_ok=True)
    print(f"\n写入 {TAURI_ICONS}")
    for size, name in TAURI_PNGS.items():
        rgba = rgba_cache.get(size) or downsample(src, SUPER, size)
        (TAURI_ICONS / name).write_bytes(png_bytes(size, rgba))
        print(f"  {name:<16} {size}x{size}")
    (TAURI_ICONS / "icon.ico").write_bytes(build_ico(items))
    print(f"  {'icon.ico':<16} 多尺寸 ICO")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
