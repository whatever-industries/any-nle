"""How Avid's Color Correction renders, as far as it has been measured, for conversion.

PROVISIONAL. Every formula here is a candidate fitted on one fixture: Avid Phase 52 "colour test"
(Media Composer 25.12.1, YCbCr 709 project, grey ramp exported to TIFF), measured by Codex in the
Avid research intake local/phase52-intake (2026-10-15; not yet promoted into AVB-0012/U-031).
Errors are in 0-255 levels on that fixture:

- Setup s: +s legal 8-bit codes (s/219 of full range), then clip. MAE ~0.006.
- Brightness b: +35 legal codes for b=30 (one magnitude measured; scaled linearly here). MAE ~0.007.
- Contrast c: affine about ~0.507 with slope 1.6325 (c=+30) / 0.6125 (c=-30); modelled as
  slope tan(45 deg + 0.45 deg * c), which fits both (the reciprocal pair) but is a candidate.
- Gain g: x * g/100, then clip. MAE ~0.2-0.3.
- Gamma y: x ** (1/y). MAE ~0.3.
- Curves: evaluated in the legal domain, u = 16 + 219 x (8-bit UI units), output
  (C(u) - 16) / 219, clipped; C is a cubic Hermite with Catmull-Rom-style secant slopes (master
  MAE 0.63, max 1.76; PCHIP was 4.4). Avid's red curve with a point at 300 clips to white from
  input ~86/255, as measured.

Not established: the order Avid applies several controls in (assumed: HSL controls in the order
setup, brightness, contrast, gain, gamma, then the Curves tab, master before red/green/blue),
whether tone controls act per RGB channel or on luma, and behaviour outside these magnitudes.
"""
from __future__ import annotations

import math

LEGAL_LOW, LEGAL_RANGE = 16.0, 219.0
CONTRAST_PIVOT = 0.507
CONTRAST_DEG_PER_UNIT = 0.45
BRIGHTNESS_CODES_PER_UNIT = 35.0 / 30.0
SAMPLES = 32  # Lumetri stores up to 32 points per curve


def _clip(v: float) -> float:
    return min(max(v, 0.0), 1.0)


def tone_transfer(tone: dict[str, float]):
    """Avid HSL Controls tone values -> a function on 0..1 (full-range normalized)."""
    steps = []
    if "setup" in tone:
        steps.append(lambda x, s=tone["setup"]: _clip(x + s / LEGAL_RANGE))
    if "brightness" in tone:
        steps.append(lambda x, b=tone["brightness"]: _clip(x + b * BRIGHTNESS_CODES_PER_UNIT / LEGAL_RANGE))
    if "contrast" in tone:
        angle = math.radians(45.0 + CONTRAST_DEG_PER_UNIT * max(min(tone["contrast"], 99.0), -99.0))
        slope = math.tan(angle)
        steps.append(lambda x, k=slope: _clip(CONTRAST_PIVOT + (x - CONTRAST_PIVOT) * k))
    if "gain" in tone:
        steps.append(lambda x, g=tone["gain"]: _clip(x * g / 100.0))
    if "gamma" in tone:
        steps.append(lambda x, g=tone["gamma"]: _clip(x) ** (1.0 / g) if g > 0 else x)

    def run(x: float) -> float:
        for step in steps:
            x = step(x)
        return x
    return run


def _hermite_slopes(xs, ys):
    """Catmull-Rom-style secant slopes: interior (y[i+1]-y[i-1])/(x[i+1]-x[i-1]), ends the
    neighbouring secant (the candidate that fitted Phase 52 best)."""
    n = len(xs)
    if n == 2:
        s = (ys[1] - ys[0]) / (xs[1] - xs[0])
        return [s, s]
    m = [(ys[i + 1] - ys[i - 1]) / (xs[i + 1] - xs[i - 1]) for i in range(1, n - 1)]
    return [(ys[1] - ys[0]) / (xs[1] - xs[0])] + m + [(ys[-1] - ys[-2]) / (xs[-1] - xs[-2])]


def curve_function(points_ui8):
    """An Avid curve (points in 8-bit UI units, as typed) -> its curve C(u) on 0..255."""
    pts = sorted(points_ui8)
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    m = _hermite_slopes(xs, ys)

    def c(u: float) -> float:
        if u <= xs[0]:
            return ys[0]
        if u >= xs[-1]:
            return ys[-1]
        i = max(j for j in range(len(xs) - 1) if xs[j] <= u)
        h = xs[i + 1] - xs[i]
        t = (u - xs[i]) / h
        h00, h10, h01, h11 = 2 * t**3 - 3 * t**2 + 1, t**3 - 2 * t**2 + t, -2 * t**3 + 3 * t**2, t**3 - t**2
        return h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1]
    return c


def curve_transfer(points_ui8):
    """An Avid curve as it acts on full-range normalized values (legal-domain evaluation)."""
    c = curve_function(points_ui8)
    return lambda x: _clip((c(LEGAL_LOW + LEGAL_RANGE * x) - LEGAL_LOW) / LEGAL_RANGE)


def sample(f) -> list[tuple[float, float]]:
    return [(k / (SAMPLES - 1), f(k / (SAMPLES - 1))) for k in range(SAMPLES)]


def lumetri_curves(tone: dict[str, float], curves_ui8: dict[str, list[tuple[float, float]]]) -> dict[str, list[tuple[float, float]]]:
    """Lumetri RGB Curves (0..1 points) reproducing the modelled Avid result, per channel:
    channel = Avid channel curve after Avid master curve after the HSL tone controls. Lumetri's
    white (master) curve is left alone: on colour patches it did not act per channel (Phase 52
    replica comparison, 2026-10-15: gain 150 on (200,150,100) gave G 200 via the master curve,
    Avid 226), while its red curve matched Avid per channel. Returns {} when nothing changes."""
    tone_f = tone_transfer(tone) if tone else None
    master_c = curve_transfer(curves_ui8["master"]) if "master" in curves_ui8 else None
    out = {}
    for name in ("red", "green", "blue"):
        channel_c = curve_transfer(curves_ui8[name]) if name in curves_ui8 else None
        if not (tone_f or master_c or channel_c):
            continue
        steps = [f for f in (tone_f, master_c, channel_c) if f]

        def run(x, steps=steps):
            for f in steps:
                x = f(x)
            return x
        out[name] = sample(run)
    return out


# --- Full colour: hue and saturation (Rec.709 chroma), then tone and curves per channel ---------
# Phase 52 best fits (Codex, provisional): Hue = rotation of Rec.709 normalized Cb/Cr by minus the
# stored angle (MAE ~0.37); Saturation = Y + s (RGB - Y) with Rec.709 luma, s = value/100 (MAE
# ~0.39). Not modelled: the small bleed into empty channels Avid shows on clipped primaries under
# Gain/Contrast (Codex patch review). Assumed order: hue, saturation, tone, master curve, channel.
KR, KG, KB = 0.2126, 0.7152, 0.0722


def hue_saturation(hue_deg: float = 0.0, saturation: float = 100.0):
    th = -math.radians(hue_deg)
    c, s_ = math.cos(th), math.sin(th)
    sat = saturation / 100.0

    def run(r, g, b):
        y = KR * r + KG * g + KB * b
        cb, cr = (b - y) / (2 * (1 - KB)), (r - y) / (2 * (1 - KR))
        u, v = cb * c - cr * s_, cb * s_ + cr * c
        rr, bb = y + 2 * (1 - KR) * v, y + 2 * (1 - KB) * u
        gg = (y - KR * rr - KB * bb) / KG
        return tuple(_clip(y + sat * (ch - y)) for ch in (rr, gg, bb))
    return run


def rgb_transform(tone: dict[str, float], saturation: float | None, curves_ui8: dict[str, list[tuple[float, float]]]):
    """The modelled Avid Color Correction as a function on full-range normalized RGB."""
    tone = dict(tone)
    hue = tone.pop("hue", 0.0)
    hs = hue_saturation(hue, 100.0 if saturation is None else saturation) if (hue or saturation not in (None, 100.0)) else None
    tone_f = tone_transfer(tone) if tone else None
    master_c = curve_transfer(curves_ui8["master"]) if "master" in curves_ui8 else None
    chans = [curve_transfer(curves_ui8[n]) if n in curves_ui8 else None for n in ("red", "green", "blue")]

    def run(r, g, b):
        if hs:
            r, g, b = hs(r, g, b)
        out = []
        for x, ch in zip((r, g, b), chans):
            for f in (tone_f, master_c, ch):
                if f:
                    x = f(x)
            out.append(x)
        return tuple(out)
    return run


def is_identity(tone, saturation, curves_ui8) -> bool:
    return not tone and saturation in (None, 100.0) and not curves_ui8


def cube_lut(f, size: int = 33) -> list[tuple[float, float, float]]:
    """size^3 output triplets, red fastest (the .cube and Premiere ITX3 order)."""
    step = 1.0 / (size - 1)
    return [f(r * step, g * step, b * step) for b in range(size) for g in range(size) for r in range(size)]
