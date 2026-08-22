#!/usr/bin/env python3
"""onto-studio app icon generator.

Design (1024x1024, Apple macOS icon grid):
  - squircle body 824x824, centered, margin 100px, corner radius 185.4 (22.5%)
  - diagonal blue->indigo->violet gradient + soft top sheen
  - white "O" ring (the Onto ring) with subtle drop shadow
  - 6 gold nodes on the ring (ObjectType) + 3 chord links forming an
    up-triangle (LinkType) -> knowledge-graph / ontology motif
  - one 4-point sparkle top-right (agent / AI capability)
The squircle shape is baked in (macOS does not mask app icons in the dock).
"""
import math

W = H = 1024
# Apple macOS Big Sur+ icon grid: 824x824 body, 100px margin, r ~= 185.4
BX, BY, BS = 100, 100, 824
R = round(BS * 0.225, 2)  # 185.4

cx, cy = 512, 512
ring_r = 252          # centerline radius of the "O" ring
ring_w = 86           # ring stroke width  -> outer 295, inner 209
bead_r = 40
chord_w = 26

# 6 beads every 60deg, starting at top (math angle 90, CCW, SVG y-down => y = cy - r*sin)
beads = []
for k in range(6):
    th = math.radians(90 - k * 60)
    bx = cx + ring_r * math.cos(th)
    by = cy - ring_r * math.sin(th)
    beads.append((bx, by))

# triangle chords connect beads 0(top), 2(lower-right), 4(lower-left) -> up-triangle, centroid at ring center
tri = [beads[0], beads[2], beads[4]]

# squircle = rounded rect (circular-arc, r=185.4) on 824 body. (G1 tangent-continuous;
# visually indistinguishable from Apple's continuous-curvature mask at icon display sizes.)
xr, yr = BX + R, BY + R          # 285.4, 285.4
x2, y2 = BX + BS, BY + BS        # 924, 924
inner = BS - R                   # 638.6  (i.e. x2 - R)
squircle = (
    f"M {xr},{BY} "
    f"H {BX+BS-R} "                       # 738.6
    f"A {R},{R} 0 0 1 {x2},{yr} "         # 924,285.4
    f"V {y2-R} "                          # 738.6
    f"A {R},{R} 0 0 1 {BX+BS-R},{y2} "    # 738.6,924
    f"H {xr} "                            # 285.4,924
    f"A {R},{R} 0 0 1 {BX},{y2-R} "       # 100,738.6
    f"V {yr} "                            # 285.4
    f"A {R},{R} 0 0 1 {xr},{BY} Z"
)

# 4-point concave sparkle, unit tip-radius 1, centered at origin
sparkle_path = (
    "M 0,-1 "
    "C 0.12,-0.30 0.30,-0.12 1,0 "
    "C 0.30,0.12 0.12,0.30 0,1 "
    "C -0.12,0.30 -0.30,0.12 -1,0 "
    "C -0.30,-0.12 -0.12,-0.30 0,-1 Z"
)
sp_cx, sp_cy, sp_s = 806, 232, 40

bead_svg = []
for (bx, by) in beads:
    bead_svg.append(
        f'<circle cx="{bx:.1f}" cy="{by:.1f}" r="{bead_r}" fill="url(#bead)"/>'
        f'<circle cx="{bx-11:.1f}" cy="{by-13:.1f}" r="9" fill="#fff" opacity="0.85"/>'
    )
bead_svg = "\n      ".join(bead_svg)

tri_path = "M " + " L ".join(f"{x:.1f},{y:.1f}" for (x, y) in tri) + " Z"

svg = f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#5B8CFF"/>
      <stop offset="50%" stop-color="#4453E6"/>
      <stop offset="100%" stop-color="#7C3AED"/>
    </linearGradient>
    <radialGradient id="glow" cx="50%" cy="22%" r="66%">
      <stop offset="0%" stop-color="#FFFFFF" stop-opacity="0.22"/>
      <stop offset="100%" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="ring" x1="0" y1="{cy-ring_r}" x2="0" y2="{cy+ring_r}" gradientUnits="userSpaceOnUse">
      <stop offset="0%" stop-color="#FFFFFF"/>
      <stop offset="100%" stop-color="#DBE3FF"/>
    </linearGradient>
    <radialGradient id="bead" cx="35%" cy="30%" r="75%">
      <stop offset="0%" stop-color="#FFF3B8"/>
      <stop offset="45%" stop-color="#F8C453"/>
      <stop offset="100%" stop-color="#E8951A"/>
    </radialGradient>
    <filter id="ringShadow" x="-30%" y="-30%" width="160%" height="160%">
      <feDropShadow dx="0" dy="12" stdDeviation="24" flood-color="#141450" flood-opacity="0.38"/>
    </filter>
    <filter id="beadShadow" x="-60%" y="-60%" width="220%" height="220%">
      <feDropShadow dx="0" dy="6" stdDeviation="8" flood-color="#3A2A00" flood-opacity="0.35"/>
    </filter>
    <filter id="softBlur" x="-100%" y="-100%" width="300%" height="300%">
      <feGaussianBlur stdDeviation="8"/>
    </filter>
    <path id="squircle" d="{squircle}"/>
  </defs>

  <use href="#squircle" fill="url(#bg)"/>
  <use href="#squircle" fill="url(#glow)"/>

  <g filter="url(#ringShadow)">
    <circle cx="{cx}" cy="{cy}" r="{ring_r}" fill="none" stroke="url(#ring)" stroke-width="{ring_w}"/>
  </g>

  <path d="{tri_path}" fill="none" stroke="#FFFFFF" stroke-opacity="0.5"
        stroke-width="{chord_w}" stroke-linejoin="round" stroke-linecap="round"/>

  <g filter="url(#beadShadow)">
      {bead_svg}
  </g>

  <circle cx="{sp_cx}" cy="{sp_cy}" r="20" fill="#FFFFFF" opacity="0.30" filter="url(#softBlur)"/>
  <path d="{sparkle_path}" fill="#FFFFFF" opacity="0.95"
        transform="translate({sp_cx},{sp_cy}) scale({sp_s})"/>
</svg>
"""

with open("onto-studio.svg", "w") as f:
    f.write(svg)
print("wrote onto-studio.svg")
