# Voi on Windows: parity with the Mac

The Windows app is a copy of the Mac app: the same windows, states, texts,
positions, colours, timings and keys. The Mac source (`apps/voi-app/src/mac/`
and what it uses) is the specification. This file is that specification,
extracted from the source with file:line references, plus the status of every
row on Windows, checked by laying the two apps' own pictures of each state
side by side (see the end of the status).

The record hotkey (Settings ▸ General) is the one thing the Mac does not have.
It is off unless someone sets it, so it changes nothing by default.

## Decisions (2026-10-19)

| # | Question | Decision |
|---|---|---|
| G1 | Dictation key | The Mac's list with Windows' names for the same keys: Right Ctrl (default, for Right ⌘), Left Ctrl, Right Alt, Left Alt; no Fn. Watched, not taken. |
| G2 | Settings window | Not the opaque macOS window: the same glass as the HUD and the dictation window, no title bar; the Mac's tabs inside the glass, its contents row for row; the Mac's red close circle in the top-right corner, and Esc or Ctrl+W; dragged by any spot that is not a control, and reopened where it was closed until Voi quits, as the Mac's is. No window has a title bar's styles, so Windows draws no close cross of its own on any glass. |
| G3 | Fonts | Cascadia Mono for SF Mono (set 5.5% larger so a line holds the same characters and wraps at the same words), Segoe UI Variable for SF Pro, at the Mac's line heights. |
| G4 | Glass | Real blur: each glass is a window of its own, blurred behind and rounded by Windows itself (blur-behind and DWM's rounded corners); the edge light and the contents drawn in it. |
| G5 | Tray icon | The Mac's dot: white (black on a light taskbar) at rest, red with eight rings breathing while recording; the Mac's menu. |
| G6–G8 | Permissions, startup, Spaces | As proposed: Windows' microphone privacy page, the Run key with Task Manager's wording, topmost on every desktop. |

## Status

| Window / state | Windows now |
|---|---|
| HUD window | Same: 458×263, glass top-right fixed 16 pt from the screen's corner, grows down-left, clicks through outside the glass; back to its first place when its saved place is on no screen |
| HUD idle / recording / open / Record? | Same geometry, colours, timings (comparison: content boxes within a point of the Mac's); transcript wraps at the same words; Copy glyph bottom-right with the green tick |
| HUD right-click menu | Same two items |
| Glass material | Acrylic blur (G4) instead of NSVisualEffectView; the Mac's edge light and meter drawn the same |
| Dictation glass | Same: 368×93, text from the top of its three lines, per-letter reveal, meter, edge light, spring in from the spot or beside the pointer, Cancel veil |
| Dictation key and delivery | Same rule (hold to talk / click on, click off, 450 ms), watched key (G1), clipboard then Ctrl+V, the Mac's two sounds; checked on a Windows PC with a real dictionary |
| Settings | Glass (G2) with the Mac's tabs, rows, texts, defaults, reset beside the distance slider, twice-a-second autosave and receipts; the record hotkey (an addition, off by default) as one extra group |
| Menu (tray) | Same items, order, titles, Ctrl shortcuts in the open menu; spinner while a job runs |
| Startup, files, quitting | Same, with Windows' places (`%APPDATA%\voi`, `%LOCALAPPDATA%\Voi\Voi.log`) |

Not the same, and why:

- The glass's blur is Windows' own, which blurs a little differently from the Mac's material, and stays under exclusive full-screen games.
- The fonts are Windows' own (G3); the letters differ, the measures do not.
- The Mac's transcript fades its top and bottom edge while scrolled; the Windows HUD cuts them.
- `Voi --check-settings` (the Mac's layout self-test) has no Windows counterpart; the Windows layout is checked by the comparison instead.

The comparison itself: `Voi --screenshots DIR` on each system draws every state into a PNG without anything going on screen (the Mac's through the views' own drawing, Windows' from the window's framebuffer); `cargo run -p voi-app --example parity -- compare MAC WIN OUT` lays each pair side by side with a difference map and reports where each one's content sits.

# Specification extracted from the Mac source

The three parts below were read out of the Mac code line by line; numbers and strings are the code's own. Where a value is AppKit's (y up), the text says so.

## Voi macOS HUD (overlay): implementation spec for the Windows clone

All paths are relative to `apps/voi-app/src/`. Mac coordinates are AppKit's (origin bottom-left, y up). Where a value is easier to use top-down, I give it in **TL** coordinates: window-local, origin top-left, y down. Every `ease(τ)` in the code is a per-frame exponential step with a fixed dt of 1/60 s: `x += (target − x)·(1 − exp(−(1/60)/τ))`.

---

## 1. Window

### Window type and flags
| Item | Value | Source |
|---|---|---|
| Class | `NSPanel`, style `Borderless \| NonactivatingPanel`, backing `Buffered`, `defer: false` | mac/overlay.rs:969-976 |
| Level | `STATUS_LEVEL = 25` (NSStatusWindowLevel: above normal windows and full-screen apps) | overlay.rs:159, 978 |
| Opaque / background | `setOpaque(false)`, `backgroundColor = clearColor` | 979-980 |
| Shadow | `setHasShadow(false)` (the glow takes the place of a shadow) | 982 |
| Drag | `setMovableByWindowBackground(true)` | 983 |
| Collection behaviour | `CanJoinAllSpaces \| FullScreenAuxiliary` | 985-988 |
| Hides on deactivate | `false` | 989 |
| Focus | `setBecomesKeyOnlyIfNeeded(true)`. The panel is never made key, and is shown with `orderFrontRegardless()`. | 991, 1417 |
| App policy | `NSApplicationActivationPolicy::Accessory`: no Dock icon, no menu bar of its own | mac/mod.rs:740 |
| View tree | content `NSView` container → `NSVisualEffectView` (blur, bottom) → `HudView` (full window frame, draws everything in `drawRect`). HudView subviews, in order: dot button, `ClipView` (holding the `NSTextView`), copy button, prompt button. The sweep `CALayer`s are sublayers of HudView's layer. | 994-1129 |

### Sizes
The window size never changes. Only the glass inside it moves.

| Constant | Value | Source |
|---|---|---|
| `PAD` | `sweep::REACH + 5.0` = 20 + 5 = **25** | overlay.rs:57, sweep.rs:34 |
| `HEADER_H` | **38** | 70 |
| `CORNER` | **11** | 71 |
| `MARGIN` (glass to screen edge) | **16** | 68 |
| `DOT_TO_EDGE` / `HIT_SIZE` / `DOT_RADIUS` / `BARS_TO_DOT` | 12 / 20 / 4.5 / 9 | 74-77 |
| `GLASS_IDLE` | `(2·(DOT_TO_EDGE + HIT_SIZE/2), HEADER_H)` = **44 × 38** | 64 |
| `GLASS_TINY` (recording, not hovered) | **78 × 38** | 66 |
| `GLASS_EXPANDED` (hovered) | **408 × 174** | 67 |
| Prompt: `PROMPT_W`, `PROMPT_H`, `PROMPT_TAIL`, `PROMPT_GAP` | 86, 26, 7, 6 | 184-188 |
| `PROMPT_ROOM` | 26 + 7 + 6 = **39** | 190 |
| `window_size(g)` | `(g.w + 2·PAD, g.h + 2·PAD + PROMPT_ROOM)` | 175-180 |
| **Window size** (always `window_size(GLASS_EXPANDED)`) | **458 × 263** | 967 |
| Glass corner radius | blur layer `cornerRadius = 11`, `masksToBounds = true` | 1011-1014 |

**Glass size formula** (overlay.rs:213-229):
```
base  = lerp(GLASS_IDLE, GLASS_TINY, tint)          // per axis
size  = lerp(base, GLASS_EXPANDED, open) * lerp(0.86, 1.0, appear)
```
All three inputs (`open`, `tint`, `appear`) are already eased, and the lerps are linear.

**Glass placement in the window** (238-246): the glass's top-right corner is fixed at `(W − 25, H − 25)`. In TL coordinates that is x = 433, y = 25.

### TL layout at full arrival (appear = 1)
| Element | Rect (x0..x1, y0..y1) |
|---|---|
| Glass, idle | 389..433, 25..63 |
| Glass, tiny | 355..433, 25..63 |
| Glass, expanded | 25..433, 25..199 |
| Header strip (top 38 pt of the glass) | y 25..63 |
| Dot hit rect (always in the same place) | 401..421, 34..54. Dot centre (411, 44): 22 pt from the glass's right edge, 19 pt from its top. |
| Copy hit rect (expanded) | 403.5..427.5, 169.5..193.5. Glyph centre (415.5, 181.5) is 17.5 pt from the glass's right and bottom edges. |
| Transcript content rect (expanded) | 33.5..424.5 (w 391), 71.5..190.5 (h 119) |
| Prompt bubble (glass idle) | 368..454, 76..102. Tail apex at (411, 69), tail base 404..418 at y 76. |

### Key formulas
- **`hit_rect`** (254-262): `x = glass.right − 12 − 20`, vertically centred in the header.
- **`wave_margin()`** (270-272): `12 + 10 − 4.5` = **17.5**.
- **`text_margin()`** (129-133): `space = 174 − 38 = 136`, `lines = floor((136 − 2·7)/lh)`, `margin = (136 − lines·lh)/2`.
  - Measured on macOS, SF Mono 14 gives `lh = 17.0`, so **lines = 7** and **margin = 8.5**.
  - `TEXT_MARGIN_MIN = 7` (83).
- **`content_rect`** (368-379): the glass below the header, inset by `margin` on all four sides. Expanded, that is **391 × 119**.

---

## 2. Placement

### First run
This applies only when no saved position exists (`restore_position`, 1717-1723, and `place_top_right`, 1740-1751).
- **Screen**: `NSScreen::mainScreen`, using its `visibleFrame` (which excludes the menu bar and Dock).
- **Window origin**:
  - `x = vis.x + vis.w − 458 − 16 + 25`
  - `y = vis.y + vis.h − 263 − 16 + 25`
- **Result**: the glass's top-right corner sits **16 pt from the top-right of the visible frame**. Placement is by the glass, not the window, because the 25 pt PAD is transparent.

### Growing
- Every size change grows **down and to the left from the fixed top-right corner** of the glass. This covers idle→tiny, →expanded, and the 0.86→1.0 arrival scale. The window itself never resizes (238-246).
- The dot and the right end of the meter are anchored to the right edge, so they never move. Tests at 1849-1879 check this.

### Drag
- `movableByWindowBackground`: you drag by pressing on the glass background.
- While the left mouse button is held, `expanded` is not updated, so the panel does not collapse mid-drag (1337-1342).

### Saving the position
- **When**: `remember_position` (1729-1738) runs every visible frame. It writes when the origin has moved by ≥ 1 pt on either axis since the last write. `saved_origin` starts at `f64::MIN`, so the first visible frame always writes, including the default placement.
- **What**: the **window** origin (the bottom-left of the 458 × 263 window) in **AppKit global screen coordinates** (bottom-left of the primary screen, y up).
- **File format**: the text `"{x} {y}"`.
- **File path**: `data_dir()/overlay-position`, where `data_dir` = `$VOI_DATA_DIR` or `dirs::data_dir()/voi` (crates/voi-core/src/recorder.rs:746-763, crates/voi-core/src/lib.rs:183-189).
- **Restore**: on the first show of the process only (`placed` flag, 1413-1416), via `setFrameOrigin(x, y)`.

### Clamping and multiple screens
- There is **no clamping** to the screen.
- There is **no screen validation** and no handling of display changes.
- Multiple screens matter only through `mainScreen` on first run, and through the global coordinate system of the saved origin.

---

## 3. States and visibility

### State machine
- Modes (state.rs:194-200): `Idle`, `Recording`, and `Stopping` ("Soniox is still flushing").
- `is_active()` = `mode != Idle` (467-469). **Stopping looks exactly like Recording**: red tint, halo, sweep, tiny size.

### When the widget shows
**`overlay_visible`** = `!hidden && (overlay_pinned || mic_in_use || is_active())` (state.rs:463-465). `hidden` outranks everything.

| Flag | What sets it | Persisted? |
|---|---|---|
| `hidden` | Menu "Hide Voi" / "Show Voi" (key `h`, `toggleHidden:`), mod.rs:175-188 | Yes, `prefs.hidden` (also re-read on settings autosave, mod.rs:509) |
| `overlay_pinned` | Menu "Keep Overlay Visible" / "Unpin Overlay" (key `o`, `toggleOverlay:`, disabled while hidden), mod.rs:190-197, 406-414 | **No**, in memory only. Defaults to false. |
| `mic_in_use` | Background thread polls `someone_else_is_listening()` **every 1200 ms** | No (mod.rs:762-785) |

**`wants_prompt`** = `mic_in_use && mode == Idle && !expanded` (state.rs:459-461).

### Visual states
| State | Glass | Contents |
|---|---|---|
| Idle, shown (pinned or mic in use) | 44 × 38 square | Red dot only, breathing slowly. No meter, no sweep. |
| Idle + mic in use, not hovered | 44 × 38 | Plus the **"Record?"** bubble below |
| Recording / Stopping, not hovered | 78 × 38 | Meter + dot with halo + edge sweep |
| Hovered (any mode, **including idle**) | 408 × 174 | Header (meter if tint > 0.01, dot), separator, transcript, copy glyph. The sweep runs only if active. |
| Hidden | Panel ordered out once appear < 0.02 | — |

`expanded` = the pointer is inside the current glass rect. It is sampled every frame, and only while no mouse button is down (1296, 1340-1342).

### Animations
All timings use `ease(τ)` with fixed dt = 1/60 unless noted.

| Value | Target | τ / rule | Effect | Source |
|---|---|---|---|---|
| `appear` | visible ? 1 : 0 | 0.14 s | Panel `alphaValue = appear`; glass scale 0.86→1.0. When not visible and < 0.02: set to 0 and `orderOut`. | overlay.rs:1378-1379, 1401-1407, 1419 |
| `open` | expanded ? 1 : 0 | 0.12 s. Snaps to target when within 0.005; no step while within 0.0005. | Glass size. Contents show when `open > 0.55` and fade in with alpha `(open − 0.55)/0.45`. The header separator switches on hard at > 0.55. | 1429-1434, 1238-1255 |
| `tint` | is_active | 0.45 s | idle→tiny size; meter colour white→red, meter alpha and height ×tint | 1313 |
| `dot_hover` | over dot | 0.09 s | `dot_scale = 1 + 0.20·hover − 0.40·press` | 1310, 1332 |
| `dot_press` | over dot && LMB down | 0.025 s pressing, 0.09 s releasing | (same) | 1309, 1312 |
| `copy_hover` | over copy (and open) | 0.09 s in, 0.16 s out | `copy_grow = 0.16·hover − 0.30·press` | 1317-1320, 1334 |
| `copy_press` | over copy && LMB | 0.025 / 0.09 s | (same) | 1320 |
| `copy_flash` | `copied_strength()` | 0.07 s rising, 0.16 s falling | Clipboard ↔ green tick crossfade | 1323-1324 |
| `copied_strength` | — | 1.0 for `HOLD = 1.1` s after copy, then linear to 0 over `FADE = 0.6` s | — | state.rs:437-447 |
| `prompt` | wants_prompt | 0.30 s | Bubble alpha. Click target enabled when > 0.5. | overlay.rs:1327, 1453-1459 |
| Sweep alpha | is_active | 0.35 s (`dt = 1/60`), snaps within 0.004 | Glow opacity ×0.30, line ×1.0 | overlay.rs:1463, sweep.rs:254-278 |
| `pulse` | — | Linear ramp with period `PULSE_PERIOD = 3.6` s | Dot breathing / halo | state.rs:87, 180-182 |
| Scroll | — | 0.18 s; the wheel applies immediately; snaps within 0.2 pt | — | overlay.rs:1677-1684 |
| Reveal | `live_chars + 5` | Per-frame fraction 0.08 (≈ τ 0.20 s at 60 fps); snaps within 0.05; clamped down if above target | — | 1475-1482 |

**Reset while hidden** (1380-1400): prompt, dot hover/press and copy hover/press/flash are set to 0, `expanded = false`, `dot_scale = 1`, `copy_grow = 0`.

**Frame loop**:
- An `NSTimer` at `FRAME_DT = 1/60` s, in `NSRunLoopCommonModes`, so it keeps firing during menus and drags (mod.rs:30, 825-832).
- Each tick runs `meter.tick(max(level_mic, level_system) if active else 0, 1/60)`, then `tick_phases`, then `overlay.refresh`, then dictation, then autosave every 30 frames and the status dot every 4 frames (mod.rs:208-244).
- A second timer at 1 s refreshes the menu (mod.rs:814-821).
- Redraws happen only when a quantised signature changes. When only the meter or the dot pulse moved, only the header strip is invalidated (overlay.rs:1504-1557).

---

## 4. Visual design

### Glass
| Item | Value | Source |
|---|---|---|
| Material | `NSVisualEffectMaterial::Popover`, `BlendingMode::BehindWindow`, `State::Active`, appearance `NSAppearanceNameVibrantDark` (forced dark) | overlay.rs:1002-1009 |
| Hairline | Rounded rect = glass inset 0.5, radius 11, stroke sRGB (1, 1, 1, **0.10**), width 1.0 | 559-564 |
| Header separator (only when open > 0.55) | Filled rect x = glass.x + 10, width = glass.w − 20, height 1 pt, at the header's bottom edge (TL y 62..63 when expanded), colour (1, 1, 1, 0.10) | 566-574 |

**Paint order**: hairline → separator → meter → dot (halo, then dot) → copy glyph → prompt (580-596).

### Record dot (679-713)
- **Colour**: always `RED` = sRGB **(1.0, 0.25, 0.22)**, alpha 1, at rest and while recording (meter.rs:15, overlay.rs:283-285). An older comment at 697 says "white at rest"; the code draws red.
- **Breathing**: `phase = 0.5 − 0.5·cos(2π·pulse)`, period 3.6 s.
- **Dot radius**: `4.5 · dot_scale · breath`, where `breath = 1 + 0.40·phase·(1 − tint) + 0.13·phase·tint`.
  - At rest it peaks at 6.3 pt; while recording at ≈ 5.09 pt (dot_scale = 1).
  - Only the size changes; brightness stays the same.
- **Halo** (only when `is_active`, not eased by tint):
  - 10 rings, drawn largest first, `t = i/9`.
  - `radius = 4.5·scale + 1 + t·(4 + 7·phase)`
  - `alpha = 0.26·(1 − t)^1.6·(0.32 + 0.68·phase)`
  - Colour RED.
- **Button**: an invisible `NSButton` covers the 20 × 20 hit rect (borderless, transparent, title `""`).

### Meter (overlay.rs:663-672, meter.rs:63-141, state.rs:59-191)
- **Shown** only when `tint > 0.01`.
- **Extent**:
  - `left = glass.x + min(17.5, 0.10·glass.w)`
  - `right = dot_hit.x − 9` (TL x = 392, fixed)
  - `mid` = header centre (TL y 44)
  - `box_h = 22`
- **Spacing**: `room = clamp((width − 45)/200, 0, 1)`, `pitch = 1.8 + 0.3·room`, `fill = 0.58 + 0.04·room`.
- **Bar count**: `slots = max(floor(width/pitch), 3) | 1` (always odd). `step = width/slots`. `bar_w = clamp(step·fill, 0.9, 2.4)`. `x0 = left + (step − bar_w)/2`.

| Size | Row width | Bars | Step | Bar width |
|---|---|---|---|---|
| Tiny | 29.2 | **17** | 1.718 | 0.996 |
| Expanded | 349.5 | **167** | 2.093 | 1.298 |

- **Source data**: 121 levels (`SPREAD = 60`, giving 2·60 + 1). The centre is live; ring k shows the centre's value k samples ago. History runs at **120 Hz**, 61 samples, so 0.5 s of history, mirrored about the middle. Floor 0.004.
- **Resampling**: each bar averages a window `1.6·span` wide, centred on `i·span`, where `span = 120/(slots − 1)`.
- **Level shaping** (state.rs:100-158):
  - `GATE = 0.26`, `voiced = max((raw − 0.26)/0.74, 0)`
  - Auto-range reference: rises instantly; otherwise decays toward `max(voiced, FLOOR = 0.34)` with `RANGE_TAU = 3.0` s
  - `shaped = clamp(voiced/max(ref, 0.34), 0, 1)^1.7` (`CONTRAST = 1.7`)
  - `want = max(shaped, 0.11·clamp(raw/0.26, 0, 1))` (`LEAK = 0.11`)
  - Centre follower: `ATTACK_TAU = 0.055` s, `DECAY_TAU = 0.09` s
- **Height**: `h = max(22·level·taper·tint, 1.0)`. Corner radius = `min(bar_w, h)/2`. All bars go into one path and are filled once.
- **Taper** (`ends_taper`, meter.rs:45-58): 1.0 out to `r = 0.55` (`SHOULDER`), then smoothstep down to `ENDS = 0.12` at the last bar. Here `r = |i − mid|/mid`.
- **Colour**: `mix(white → RED, tint)` with alpha = `tint`. Every bar has the same colour.

### Edge sweep (sweep.rs, driven at overlay.rs:1350-1357, 1443-1447, 1463)
- **Runs** only while `is_active`, fading in and out over τ 0.35 s.
- **Track**: the glass inset by 0.5, corner radius 11.
- **Two passes**, each a host `CALayer` with an image mask and a radial `CAGradientLayer` "blob":
  - **Glow** (opacity × `GLOW_ALPHA = 0.30`), drawn under the Line.
  - **Line** (opacity × 1.0).
- **Blob**:
  - Colour sRGB **(1.0, 0.30, 0.24)**. This differs slightly from RED.
  - Radial stops (position, alpha): (0, 1.00), (0.25, 0.74), (0.50, 0.32), (0.75, 0.07), (1.00, 0.00).
  - Square of side `perimeter·0.60` (`SPAN`): **125.5 pt** tiny, **684.7 pt** expanded.
- **Motion**: `CAKeyframeAnimation` on `position` along the outline path, calculation mode `kCAAnimationPaced` (constant speed), repeating forever. The phase is carried across rebuilds via `timeOffset = phase·lap`.
- **Lap time**: `3.4 · max(P/P_ref, 1)^0.35`, with `P_ref` = perimeter of 78 × 38 at r = 11 = 213.1.
  - Tiny track (77 × 37): P = 209.1, lap **3.4 s**.
  - Expanded track (407 × 173): P = 1141.1, lap ≈ **6.12 s**.
- **Rebuild thresholds**: the animation is rebuilt when the perimeter changes by > 0.5 pt; the mask is repainted when it changes by > 2 pt.
- **Mask images** (282-309):
  - **Line**: the outline stroked black at width `CORE = 1.4`.
  - **Glow**: 56 strokes (`STEPS`). For `i = 0..55`: `r = 20·(1 − i/55)`, `alpha = 0.17·exp(−(r/4)²)` (`FALLOFF = 4`), `width = 2r + 1.4`. This gives a Gaussian falloff that reaches ±20 pt (`REACH`) across the border, both outward into PAD and inward over the glass.

### Transcript
- **Font**: `NSFont::monospacedSystemFontOfSize_weight(14, 0)`, which is SF Mono Regular (`.AppleSystemUIFontMonospaced-Regular`) (1793-1803).
- **Metrics** (measured on macOS):
  - Line height `defaultLineHeightForFont` = **17.0**
  - Ascender 13.535, cap height 9.864, descender −2.953
  - Advance 8.654, so ≈ **45 characters per 391 pt line**
- **Colours** (1807-1812):
  - Settled text: sRGB **(0.95, 0.96, 0.98, 1.0)**
  - Live text (still being revised): **(0.74, 0.77, 0.81, α)**
- **Runs** (state.rs:365-420):
  - The last `VIEW_LINES = 200` settled lines, as `"You: …\n"` or `"Them: …\n"`. Labels come from voi-audio `Origin::label`: Mic = `"You"`, System = `"Them"`.
  - Then the live lines, ordered by start time. Each is the label + `": "` (settled), the finalized words (settled), and the tail (live).
  - Trailing newlines are stripped.
- **Fade-in at the reveal head** (`faded`, 1759-1785):
  - For live character n (counted across all live runs): `α = clamp((reveal − n)/FADE, 0, 1)`, with `FADE = 5` characters.
  - α is quantised to steps of 0.1. Characters with α ≤ 0 are not shown at all.
  - Settled runs mixed in among the live ones stay at α 1.
- **Updates**: only the storage after the settled head is replaced each frame (1566-1619).
- **Text view**:
  - Not editable, **not selectable**, no background, zero container inset.
  - Fixed width = expanded content width (391), so lines never re-wrap while the panel grows (1067-1092).
  - Shown only while `open > 0.55`. While closed nothing is laid out, and the scroll is marked stale; the next open jumps straight to the end.
- **Visible lines**: **7** lines of 17 pt in the 119 pt content area, with an 8.5 pt margin on all four sides.
- **Vertical nudge**: `text_nudge = ((−desc) − (asc − cap))/2 − 1.6` = **−1.959**. The text frame's y is `clip.h − doc + scroll_y − nudge`, so the text sits ≈ 1.96 pt higher than strict top alignment (104-120, 1693-1696).
- **Scrolling** (316-360, 1648-1714):
  - `bottom = max(docHeight − 119, 0)`.
  - **Pinned** (following the newest line): it follows growth immediately. A shrink is accepted only after 15 consecutive frames (`SETTLED`).
  - **Wheel**: `target = clamp(target − wheel, 0, bottom)`, applied immediately. It re-pins when within 1 pt of the bottom (`AT_END`).
  - **Not pinned**: the target is only clamped down, if the document shrinks below it.
  - Easing toward the target uses τ 0.18 s.
- **Edge fades**: a `CAGradientLayer` mask on the clip view, start (0.5, 1) to end (0.5, 0).
  - Locations `[0, 0.045, 1 − arriving, 1]`, where `arriving = clamp(17/119, 0.06, 0.35)` = 0.1429.
  - Colours `[top, 1, 1, bottom]`.
  - The top end is alpha 0 only while `scroll_y > 1`; the bottom end only while `scroll_y < bottom − 1`. Otherwise alpha 1 (1094-1116, 1700-1711).
  - At 119 pt that is a 5.4 pt fade at the top and 17 pt at the bottom.

### Copy control (613-648, 823-875)
- **Position**: the bottom-right corner of the glass, floating over the transcript.
  - Glyph centre 17.5 pt from the right and bottom edges (`COPY_INSET = 11` + `COPY_GLYPH/2 = 6.5`).
  - Hit target 24 × 24 (`COPY_SIZE`), centred on the glyph.
  - An invisible `NSButton` covers it (title `""`).
- **Visibility**: `fade = clamp((open − 0.55)/0.45, 0, 1)`.
- **Clipboard glyph**:
  - Box side `13·(1 + copy_grow)`, centred in the target.
  - Colour sRGB **(0.70, 0.73, 0.78, fade·(1 − flash)·0.55)**.
  - Two rounded pages, each 0.72·side square. Line width 0.10·side (1.3 pt), corner 0.16·side (2.08 pt).
  - The front page is at fractional (0, 0) (bottom-left); the back page at (0.28, 0.28) (upper-right).
  - The back page is clipped (even-odd) to exclude the front page grown by one line width (radius corner + line), which leaves a gap between the two.
- **Tick glyph**:
  - Fixed 13 pt box (it does not grow).
  - Polyline (0.12, 0.54) → (0.38, 0.24) → (0.88, 0.78) in bottom-left fractional coordinates.
  - Width 0.14·side (1.82 pt), round cap and join.
  - Colour sRGB **(0.32, 0.80, 0.44, fade·flash)**.
- **Labels**: there are none. No "Copy" or "Copied" text is drawn; the confirmation is the tick holding 1.1 s, then fading over 0.6 s.

### Prompt bubble (716-814)
- **Text**: `"Record?"`, `systemFontOfSize(11.5)` regular, centred, colour sRGB **(0.94, 0.95, 0.97, α)**.
  - α is quantised to 1/24 steps and the attributed string is cached per step.
  - Measured text size is 44.6 × 14.0. It is centred vertically at `y = bubble.y + (26 − lineH)/2`.
- **Bubble**:
  - 86 × 26, centred horizontally on the dot centre.
  - Its top edge sits 13 pt below the glass bottom: 6 pt gap + 7 pt tail.
  - Corner radius 7.
- **Tail**: one continuous outline. Apex at (dot centre x, bubble top + 7); base half-width 7.
- **Paint**: fill sRGB **(0.13, 0.13, 0.15, 0.94·α)**; stroke (1, 1, 1, 0.12·α), width 1.
- **Click**: an invisible `NSButton` covers the bubble rect, enabled while prompt > 0.5. It sends `toggle:`.

---

## 5. Mouse and keyboard behaviour

| Input | Behaviour | Source |
|---|---|---|
| Click dot | `toggle:`: Idle → start recording, Recording → stop (→ Stopping), Stopping → nothing | overlay.rs:1177-1178, mod.rs:88-92, engine.rs:33-40 |
| Click "Record?" bubble | Same `toggle:` | overlay.rs:1182-1183 |
| Click copy (only when open > 0.55) | `copyTranscript:` puts **the whole history** (`runs(usize::MAX)`, trimmed) on the general pasteboard and sets `copied_at = now`. With no text at all it copies `"Listening…"` (active) or `"Not recording"` (idle). | mod.rs:157-168, state.rs:424-434 |
| Click elsewhere on the glass | No action. It drags the window (movable by background). | 983 |
| Clicks in the PAD margin, or anywhere outside the glass / active prompt | `hitTest:` returns nil, so the click falls through to whatever is behind | 462-476 |
| Hover | No delay. The panel expands while the pointer is inside the current glass rect and collapses when it leaves (open eases at τ 0.12 s). Sampled every frame from `NSEvent::mouseLocation`; no tracking areas. Frozen while any left button is held. | 1290-1296, 1340-1342 |
| Dot / copy hover and press | Grow and dip as in §3. Press is detected with `pressedMouseButtons() & 1`. | 1289-1334 |
| Cursor | Forced to `NSCursor::arrowCursor()` every frame while over the glass | 1302-1304 |
| Scroll wheel | Over the transcript clip view. Trackpad (`hasPreciseScrollingDeltas`) deltas are in points; a wheel notch counts as `dy × 17` pt. Accumulated and applied once per frame. Positive dy moves toward earlier text and un-pins. | 509-515 |
| Right-click | A menu appears **only when the click is in the header strip** (top 38 pt of the glass). Items: **"Open Current Session"** (`openLastTranscript:`, opens the newest transcript file) and **"Open Sessions Folder"** (`openSessions:`, creates and opens `data_dir/sessions`). No key equivalents. Elsewhere `menuForEvent` returns nil. | 438-458, 1192-1205, mod.rs:95-109 |
| Keyboard | The overlay handles none (it never takes key focus). Overlay-related menu-bar items: "Keep Overlay Visible"/"Unpin Overlay" (⌘O), "Hide Voi"/"Show Voi" (⌘H). The initial title "Show Overlay" is replaced on the first refresh. | mod.rs:326-339, 406-417 |

---

## 6. Every user-visible string in overlay.rs
- `"Record?"` (743)
- `"Open Current Session"` (1193)
- `"Open Sessions Folder"` (1194)
- `""`: the titles of the three invisible buttons (1034, 1054, 1061)

Not literally in overlay.rs, but rendered or produced by it:
- `"You"` and `"Them"` with `": "` (voi-audio lib.rs:130-135, state.rs:369, 398)
- Copy fallbacks `"Listening…"` and `"Not recording"` (state.rs:428-430)

Comments that do not match the code: the "Copied ✓" text (overlay.rs:143, mod.rs:166) is no longer drawn. The pulse is "~2.4s" in a comment (state.rs:70) and "three and a half seconds" in another (overlay.rs:1518); the actual constant is 3.6 s.

---

## 7. Hard to reproduce on Windows
- **Behind-window blur** (`NSVisualEffectView` Popover / BehindWindow / VibrantDark) applied only to an **inset, rounded (r = 11), animating** rectangle inside a larger transparent window. The blur region changes size every frame while 25 pt of transparent margin stays around it.
- **Per-pixel-alpha borderless window** with **click-through outside the glass** (`hitTest` → nil) and no system shadow.
- **Non-activating panel at status level 25** that never takes focus, joins all Spaces, sits over full-screen apps, and has no taskbar/Dock presence (Accessory policy).
- **Core Animation pieces**:
  - The sweep `CAKeyframeAnimation` along a `CGPath` with paced timing, running on CA's clock rather than the frame loop.
  - A radial `CAGradientLayer` blob, masked by a pre-rendered alpha image of 56 Gaussian strokes.
  - Implicit animations disabled with `CATransaction.setDisableActions`.
- **`CAGradientLayer` alpha mask** on the transcript clip, giving variable top and bottom fades that are recoloured, not hidden.
- **Global pointer sampling every frame** (hover with no focus or tracking area) and forcing the arrow cursor over another app's cursor regions.
- **Movable-by-background** drag on a non-activating window.
- **Precise trackpad deltas vs line-based wheel deltas.**
- **SF Mono and AppKit text metrics** (line height 17.0, the cap-height/descender nudge, `NSLayoutManager` used-rect height) set the 7-line, 8.5 pt-margin layout. A different font changes `text_margin`, the nudge and the wrap width (≈ 45 characters).
- **AppKit bottom-left global coordinates** in the saved `overlay-position` file (window origin, not glass), so the file is not portable between platforms. `apps/voi-app/src/win/hud.rs` already uses the same file through its own coordinates.
- **Retina-scaled rendering** of the stroked glyphs and the block-based mask images; sub-pixel bars down to 0.9 pt wide.
## Voi Settings window (macOS): exact specification for the Windows clone

Sources: `apps/voi-app/src/mac/settings.rs` (S), `apps/voi-app/src/mac/spotgrid.rs` (G), `apps/voi-app/src/mac/login.rs` (L), `crates/voi-core/src/prefs.rs` (P), `apps/voi-app/src/mac/mod.rs` (M), and `crates/voi-core/src/recorder.rs` / `lib.rs` for where the key and files are stored.

I checked the layout numbers by running the existing debug binary with `target/debug/voi-app --check-settings`. It builds the window without showing it. Output: `General: 150 pt to spare`, `Audio: 243 pt to spare`, `Dictation: 16 pt to spare`, `Dictionary: 20 pt to spare`. From these I worked out the real height of each hint text (shown below as "measured").

---

## 0. Coordinate conventions

AppKit counts y upward from the bottom-left. Below, every position is converted to **top-down coordinates inside the 560×500 content area** (x from the left, "top" measured from the top edge), which is how Windows/WPF lays things out. The raw AppKit formulas are cited by line.

### Layout constants (S:44-58)
| Const | Value | Meaning |
|---|---|---|
| `WIDTH` | `560.0` | content width of every tab |
| `HEIGHT` | `500.0` | content height of every tab |
| `PAD` | `20.0` | window edge to group |
| `GROUP` | `WIDTH - 2*PAD` = **520** | group width |
| `INSET` | `16.0` | group's left edge to row label (and the separator's left inset) |
| `INSET_RIGHT` | `14.0` | group's right edge to control |
| `ROW` | `44.0` | ordinary row height |
| `FIELD` | `290.0` | wide right-hand control width |
| `FOOTER` | `44.0` | space reserved for the footer at the bottom |

### Layout primitives (class `Page`, S:64-220)
- **Cursor** starts at `HEIGHT - PAD`, i.e. top-down **20** (S:78).
- **`header(text)`** (S:86-92): label 13 pt **semibold** (`NSFontWeightSemibold`), default label colour. Frame x = `PAD+4` = **24**, width `GROUP` = 520, height **17**. Cursor moves down `17 + 8` = **25**.
- **`begin_group()`** (S:95-109): `NSBox` of type Custom, no title, **corner radius 10.0**, **border width 0.5**, border colour `NSColor::separatorColor()`, fill `NSColor::quinarySystemFillColor()`. It sits behind the rows (added `Below`). Its frame is fixed in `end_group`.
- **`row(label, height)`** (S:113-139):
  - From the second row of a group on, a hairline separator (`NSBoxType::Separator`) is drawn at the row's top edge. AppKit frame: x = `PAD+INSET` = **36**, y = top − 0.5, width `GROUP-INSET` = **504**, height 1. It runs to the group's right edge.
  - Label: 13 pt regular, default label colour. x = **36**, width `GROUP - INSET - FIELD - INSET_RIGHT - 8` = **192**, height **17**, vertically centred (top = row middle − 8.5).
  - Cursor moves down `height`. The function returns the row's middle.
- **`right(size, middle)`** (S:153-158): the control is right-aligned with its right edge at `PAD + GROUP - INSET_RIGHT` = **x 526**, and vertically centred on the row middle.
- **`end_group()`** (S:141-149): group box frame x = 20, width 520, from the group's top down to the cursor. Then cursor moves down **6**.
- **`hint(text)`** (S:161-172): wrapping label, **11 pt**, `secondaryLabelColor`, not selectable. Width `GROUP-8` = **512** (also its preferred max layout width), x = **24**. Height = fitting height, rounded up. Cursor moves down `height + 18`.
- **`into_tab(title, symbol)`** (S:204-219): an `NSViewController` with title = tab name and `preferredContentSize` 560×500. The `NSTabViewItem` gets label = title and image = SF Symbol `symbol`, with accessibility description = title.

---

## 1. Window

| Aspect | Value (source) |
|---|---|
| Class | Plain `NSWindow`; content view controller is an `NSTabViewController` (S:477, 488-500) |
| Style mask | `Titled \| Closable \| Miniaturizable` (S:485-487). **Not** resizable. **No** full-size content view, **no** transparent titlebar, nothing hidden from the titlebar. |
| Backing | `NSBackingStoreType::Buffered`, defer `false` (S:493-494) |
| Size | content 560 × 500 (`setContentSize`, S:503), the same on every tab (S:42-45, 208) so switching tabs never resizes the window |
| Min/max size | None set; the window can't be resized anyway |
| Position | `window.center()` once, when the window is created (S:504). Not remembered: no frame autosave name. The `Settings` object is created lazily on the first "Settings…" and then reused (M:144-150), and the window has `setReleasedWhenClosed(false)` (S:499). So within one app run a dragged window reopens where it was left; after a relaunch it is centred again. |
| Title text | Never set explicitly. Standard AppKit behaviour (not in this code): the window title follows the content view controller's title, which a tab view controller takes from the selected tab. So the title reads **"General" / "Audio" / "Dictation" / "Dictionary"**. |
| Toolbar style | `NSWindowToolbarStyle::Preference` (S:502). Comment: "Icons over their names, centred: the look of a Mac app's settings." (S:501) |
| Tab presentation | `NSTabViewControllerTabStyle::Toolbar` (S:478): toolbar buttons with icon above label |
| Tabs and SF Symbols (S:479-482) | 1. **General**: `gearshape`; 2. **Audio**: `mic`; 3. **Dictation**: `command`; 4. **Dictionary**: `character.book.closed` |
| Background / material | Nothing set: default window background (`windowBackgroundColor`). No vibrancy or visual effect view. |
| Window corner radius | System default (not set) |
| Groups | Radius 10, 0.5 px `separatorColor` border, `quinarySystemFillColor` fill (S:101-104) |
| Opening | Menu bar item **"Settings…"** with key equivalent **","** (⌘,) (M:343). `show()` refills every field from disk, then `makeKeyAndOrderFront` and `NSApplication.activate()`. The app is an Accessory (menu-bar) app and must raise itself (S:597-602). |
| Selected tab memory | No code for it. The tab controller keeps the last tab while the app runs; it is not stored across launches. |

---

## 2. Tabs, row by row

Each tab's view is 560×500. Top-down coordinates. "Switch" = `NSSwitch` sized to `fittingSize()` (the system size; no number in the code).

### Tab 1: General (`gearshape`) (S:264-305)

| Top-down y | Element |
|---|---|
| 20–37 | Header **"Soniox"** |
| 45–133 | Group (height 88) |
| 45–89 (middle 67) | Row **"API key"**: `NSSecureTextField`, frame x 236, top 56, **290×22**. Placeholder **"Paste your key"** (S:276). |
| 89 | separator |
| 89–133 (middle 111) | Row **"Languages"**: `NSTokenField`, x 236, top 100, **290×22**. No placeholder, no tokenising settings (defaults, so a comma ends a token). |
| 139–167 | Hint (measured 28 pt, 2 lines): "Get a key at console.soniox.com — it is stored on this Mac only. Languages are the ones that may turn up in your audio, as names or codes: English, uk, ru." (S:289-292) |
| 185–202 | Header **"Startup"** |
| 210–254 | Group (one row) |
| 210–254 (middle 232) | Row **"Launch at login"**: Switch, right edge x 526 |
| 260–288 | Hint (28 pt): "Voi starts in the menu bar when you log in, so the dictation key works from the first minute. The same switch is in System Settings ▸ General ▸ Login Items." (S:300-303) |
| 306 | end of content; 150 pt spare above the footer |

Fields and defaults:
- **API key**: not in `Prefs`. Shows `find_api_key()` on open (S:539-542). Saved to `config.env` (see §4). No default.
- **Languages**: `Prefs.languages`, default `["uk","ru","en"]` (P:18-20), shown as `join_list`, i.e. `"uk, ru, en"` (P:399-401). On save: `parse_list(stringValue)` (splits on `,` `\n` `;`, trims, drops empties and duplicates; P:386-396), then each item goes through `language_code`. That lowercases and maps 24 names to codes (english→en, ukrainian→uk, українська→uk, russian→ru, polish→pl, german→de, french→fr, spanish→es, italian→it, portuguese→pt, dutch→nl, czech→cs, slovak→sk, turkish→tr, arabic→ar, hebrew→he, chinese→zh, japanese→ja, korean→ko, hindi→hi, romanian→ro, bulgarian→bg, swedish→sv, norwegian→no). Anything else passes through lowercased (P:410-442). No validation text. The field is not rewritten after saving: "English" stays on screen as typed until the window is reopened.
- **Launch at login**: not in `Prefs`; it is macOS state (see §5).

### Tab 2: Audio (`mic`) (S:307-333)

| Top-down y | Element |
|---|---|
| 20–37 | Header **"Input"** |
| 45–133 | Group |
| 45–89 (middle 67) | Row **"Microphone"**: `NSPopUpButton` (pullsDown false), x 236, top 54, **290×26** |
| 89 | separator |
| 89–133 (middle 111) | Row **"Start and stop sounds"**: Switch |
| 139–195 | Hint (measured 56 pt, 4 lines): "Used for dictation and for your side of a call. AirPods switch from music to headset mode whenever their microphone opens, which interrupts what is playing and takes a second or two — choose the MacBook's microphone to keep them for listening only. Dictation sounds tell you it heard the key while you look at what you are typing into." (S:325-331) |
| 213 | end; 243 pt spare |

- **Microphone** → `Prefs.microphone: Option<String>`, default `None` (P:312). Items in order (S:608-629):
  1. `"System default ({name})"`, or `"System default"` when no default device is known. Maps to `None`.
  2. Each input device name from `voi_audio::inputs()`, excluding the app's own `"Voi System Tap"` (mic.rs:30, 43).
  3. If the saved device is absent: `"{chosen} (not connected)"`, appended last and kept selectable.
  
  The selection falls back to index 0. While the window is open the list is rebuilt live whenever Core Audio reports a device or default-device change, keeping the current choice (S:709-712).
- **Start and stop sounds** → `Prefs.sounds`, default `true` (P:312).

### Tab 3: Dictation (`command`) (S:335-423)

| Top-down y | Element |
|---|---|
| 20–37 | Header **"Shortcut"** |
| 45–133 | Group |
| 45–89 (middle 67) | Row **"Key"**: `NSPopUpButton`, x 326, top 54, **200×26** |
| 89 | separator |
| 89–133 (middle 111) | Row **"Mode"**: `NSPopUpButton`, x 326, top 98, **200×26** |
| 139–167 | Hint (28 pt): "Speak while it listens; it is typed where the cursor is when it stops. Let go with the pointer on the window to throw it away instead." (S:367-368) |
| 185–202 | Header **"Window"** |
| 210–388 | Group (height 178) |
| 210–254 (middle 232) | Row **"Follow pointer"**: Switch |
| 254 | separator |
| 254–344 (middle 299) | Row **"Position on screen"**, row height `spotgrid::SIZE.height + 20` = **90** (S:378). Spot grid **112×70** at x 414, top 264 |
| 344 | separator |
| 344–388 (middle 366) | Row **"Distance from the edge"**: `NSSlider` x 326, top 355, **200×22**, min **6.0**, max **220.0** (S:382-390); reset button **20×20** at x = slider.x − 26 = **300**, top 356 (S:393-401) |
| 394–422 | Hint (28 pt): "Above the pointer and to its right; left of it or under it where the screen runs out. Never on it — the pointer on the window throws a dictation away." (S:419-420) |
| 440 | end; 16 pt spare |

Details are in §6.

### Tab 4: Dictionary (`character.book.closed`) (S:425-475)

| Top-down y | Element |
|---|---|
| 20–37 | Header **"Words to expect"** |
| 45–73 | Hint, placed straight under the header with **no group in between** (28 pt): "Names, jargon and product names that come up in your calls. Commas or new lines between them. A hint, not a filter: nothing is forced to match." (S:429-430) |
| — | Cursor moves back up 8 (`dictionary.y += 8.0`, S:432), so the box starts 10 pt below the hint |
| 83–430 | Group box, x 20, w 520, height 347. Bottom edge fixed at AppKit `FOOTER + 26` = 70 (S:434), so the box fills to the footer whatever the hint's height. |
| 89–424 | `NSScrollView` inset 6 on every side of the box: x 26, width `GROUP-12` = **508**, height box − 12 = 335 (S:438-441). Vertical scroller on, no background (S:444-445). |
| — | Inside it an `NSTextView`: plain text (`setRichText(false)`), no background, **13 pt** system font, text container inset **(4, 6)**, wraps to width (S:446-463). No placeholder. |
| 436–450 | Count label: x **280**, width **256** (right edge 536), height 14, right-aligned, `monospacedDigitSystemFont` **11 pt**, weight 0.0, `secondaryLabelColor`; **`systemRedColor`** when over the limit (S:465-473, 664-669) |
| — | Spare 20 pt |

- Field: `Prefs.terms: Vec<String>`, default empty (P:305). Shown as `join_list`, i.e. `", "`-separated on a single line; stored as `parse_list(text)`.
- Count text (S:658-663): **`"{n} term{s} · {used} of about 10000 characters"`**. `s` is empty when n == 1. `used` = Σ(characters of each term + 2). `MAX_CONTEXT_CHARS = 10_000` (P:15) is printed without a thousands separator. It updates on every poll, about every 0.5 s.

### Footer, the same on every tab (S:176-201)
- **Status label**: wrapping label, 11 pt, `secondaryLabelColor`, not selectable, **max 2 lines**, truncates at the tail. AppKit frame (24, 10, `GROUP-190` = **330**, `FOOTER-16` = **28**), i.e. top-down x 24, top 462–490.
- **Build label**: 10 pt, `tertiaryLabelColor`, right-aligned. AppKit (`PAD+GROUP-180` = **360**, 10, **176×14**), i.e. top-down x 360–536, top 476–490. Text is `version_line()`.
- No separator line and no background behind the footer.
- All four footers always show the same message: `report()` writes to every one (S:805-809).

---

## 3. Footer contents and timing

**Version line** (M:616-626):
- Bundled build: **`"Voi {CFBundleShortVersionString} · build {CFBundleVersion} · {VoiCommit}"`**
- Without the bundle keys: **`"Voi {version} (unpackaged)"`**, where version falls back to `CARGO_PKG_VERSION`.

**Status message on open** (S:589-595):
- If Accessibility is granted (`AXIsProcessTrusted`): the full path of `settings.json`, e.g. `~/Library/Application Support/voi/settings.json` (expanded).
- Otherwise: **"Voi needs Accessibility to type for you — System Settings ▸ Privacy & Security ▸ Accessibility. Until then dictation copies to the clipboard."**

**Messages from the autosave poll** (S:702-779, 783-803):
- `"Key saved."`
- `"Could not save the key: {e}"`
- The launch-at-login messages (§5)
- `"The dictionary is {count} characters; Soniox takes about 10000. Not saved."`
- `"Saved: {languages}, {n} term{s}, {microphone}, {key label} {placement}, {margin:.0}pt in, sound {on|off}."`
  - `{languages}`: `join_list`, or `"any language"` when the list is empty
  - `{microphone}`: the device name, or `"default microphone"`
  - `{placement}`: `"beside the pointer"` when Follow pointer is on, otherwise `Spot::label().to_lowercase()` (e.g. "bottom centre")
  - Defaults example: `Saved: uk, ru, en, 0 terms, default microphone, Right ⌘ bottom centre, 34pt in, sound on.`
- `"Could not save: {e}"`

**Timing**: messages never time out or fade. One stays until the next message replaces it. The poll returns `None` (no change to the text) when nothing changed.

---

## 4. Save model

- **No Save button.** A 60 fps frame timer (`FRAME_DT = 1.0/60.0`, M:30) runs in common run-loop modes (M:822-830). On every 30th frame (**every 0.5 s**) it calls `autosave_settings()` (M:229-234), which only does anything while the window is visible (S:703-705).
- **Order inside `autosave()`** (S:702-755). Each step that produces a message returns straight away, so the remaining steps run on the next poll:
  1. If the microphone list changed: rebuild the popup.
  2. `tidy()`: margin reset button and dictionary counter.
  3. **Key**, only if the field is not being edited (`key.currentEditor().is_none()`, i.e. focus has left it), the trimmed text is non-empty, and it differs from the key last seen. Writes and returns `"Key saved."`. Clearing the field never deletes the key.
  4. **Launch at login**, if the switch differs from what macOS last said (§5).
  5. **Snapshot** of every control into `Prefs` (S:673-693). The base is the last-saved `Prefs`, so fields with no control here (`hidden`, `record_hotkey`) are carried over unchanged. If it equals the last write: nothing happens.
  6. **Dictionary limit**: if Σ(len + 2) > **10000**, return the "Not saved." message. Nothing at all is saved, including unrelated changes, and the message repeats every poll until the dictionary is shortened.
  7. `prefs::save()`: pretty-printed JSON. On success, return the "Saved: …" receipt and remember what was written.
- **Where things go**:
  - `data_dir()` = `~/Library/Application Support/voi` (macOS) or `%APPDATA%\voi` (Windows); overridable with the `VOI_DATA_DIR` environment variable (lib.rs:183-190).
  - `settings.json` there (P:350-352) holds `Prefs`, serde field names, enums in kebab-case.
  - The key goes to **`config.env`** as `SONIOX_API_KEY={trimmed key}\n`, chmod **600** on Unix; on Windows `acl::owner_only` (recorder.rs:727-743).
  - Reading the key (recorder.rs:767-796): environment variable `SONIOX_API_KEY` first, then `config.env`, then `./.env`. The value is trimmed and surrounding `"` are removed.
- **After any message** the main loop reloads `prefs::load()` into its running copy and pushes `hidden` to shared state (M:494-512).
- **Reopening** (`show()`) refills every control from disk. Anything not saved, such as an over-limit dictionary, is replaced by the stored value.
- **Validation texts**: only the dictionary limit (above) and the save errors. Languages and the API key format are not validated.
- **`Prefs` fields and defaults** (P:246-318): `languages ["uk","ru","en"]`, `terms []`, `hidden false`, `dictation_mode push-to-talk`, `dictation_spot bottom-center`, `dictation_follow_pointer false`, `dictation_key right-command`, `dictation_margin 34.0`, `sounds true`, `microphone null`, `record_hotkey none` (Windows-only, the record key; no control in the Mac window).
- **`--check-settings`** (M:704-716): builds `Settings::new` without showing it, prints `"{tab}: {room:.0} pt to spare"` for each tab, and exits 1 if any value is below 0.

---

## 5. Launch at login (L, S:760-779)

- State comes from macOS `SMAppService.mainAppService.status`, never from `settings.json`. Raw 1 → `On`, 2 → `NeedsApproval`, anything else (0, or 3 NotFound) → `Off`. No `SMAppService` class → `Unavailable` (L:43-57).
- The switch shows **on** for `On` or `NeedsApproval` (`wanted()`, L:33-35).
- When the user flips it, `login::set(wanted)` runs. The message:
  - `On` → **"Voi will launch at login."**
  - `Off` → **"Voi will not launch at login."**
  - `NeedsApproval` → **"Allow Voi in System Settings ▸ General ▸ Login Items to finish turning on launch at login."**
  - `Unavailable` → **"This Mac has no Login Items service."**
  - Error → **"Could not change launch at login: {e}."**, where `{e}` is one of:
    - `"only the installed Voi.app can launch at login, not a bare binary"` (turning on when the executable path lacks `.app/Contents/MacOS/`)
    - `"this Mac has no Login Items service"`
    - the `NSError` `localizedDescription`
  - In practice `set()` returns the error above instead of `Ok(Unavailable)` when there is no service.
- Afterwards the switch is set back to what macOS actually reports (S:775-777), so a refused change snaps back.
- CLI: `Voi --launch-at-login on|off|status` prints `On` / `Off` / `NeedsApproval` / `Unavailable`. A bad argument prints `expected on, off or status, not "{other}"` and exits 1 (M:717-731).

---

## 6. Dictation tab in detail

- **Key popup** (`DictationKey::ALL`, P:63-81), in order: **"Right ⌘"** (default), **"Left ⌘"**, **"Right ⌥"**, **"Left ⌥"**, **"Right ⌃"**, **"Fn (Globe)"**. Writes `dictation_key` (`right-command`, `left-command`, `right-option`, `left-option`, `right-control`, `fn`). An unknown saved value selects index 0. Masks are at P:87-96.
- **Mode popup** (S:361): **"Hold to talk"** (index 0, `push-to-talk`, default) and **"Click on, click off"** (index 1, `toggle`). Writes `dictation_mode`.
- **Follow pointer**: Switch, writes `dictation_follow_pointer`, default `false`. Turning it on does **not** disable or dim the spot grid. Placement rule (P:194-228): 10 pt above-right of the pointer tip (`BESIDE_POINTER = 10.0`); flips left when there's no room on the right; goes under when there's no room above, using `UNDER_POINTER = 28.0`; then clamped to the usable screen area.
- **Spot grid** (G), writes `dictation_spot`:
  - Custom `NSView` named `VoiSpotGrid`, `SIZE` **112×70** (G:16).
  - Frame: rounded rect inset 0.5 (so 111×69), **radius 6.0**. Fill sRGB **(0.5, 0.5, 0.55, α 0.20)**, stroke sRGB **(0.5, 0.5, 0.55, α 0.45)**, line width **1.0** (G:97-109).
  - Dots: 3×3 grid, centres **14** pt (`INSET`) in from each edge, horizontal step (112−28)/2 = **42**, vertical step (70−28)/2 = **21**. The top row comes first in `Spot::ALL` order (G:81-91).
  - Unchosen dot diameter **5.0**, colour sRGB **(0.55, 0.57, 0.62, α 0.85)**. Chosen dot diameter **9.0**, colour `controlAccentColor` (G:17-18, 115-125).
  - Clicking (`mouseDown`) picks the **nearest** dot to the click point; there's no hit-testing (G:45-58). No drag tracking, hover, keyboard support or tooltip.
  - Default index **7** = BottomCenter (G:64, 73).
  - Spot order and labels (P:118-142): Top left, Top centre, Top right, Middle left, Centre, Middle right, Bottom left, **Bottom centre** (default), Bottom right. Stored as kebab-case: `top-left` … `bottom-right`, centre = `center`, `bottom-center`.
- **Distance from the edge** → `dictation_margin`, units **points**, default `DEFAULT_MARGIN = 34.0` (P:299):
  - Plain `NSSlider`, range **6.0–220.0**, no tick marks, no number shown, no default mark on the track.
  - **Reset button** (S:393-416): `PushOnPushOff`, no border, image only, SF Symbol **`arrow.counterclockwise`** with accessibility description **"Reset"**, tooltip **"Back to 34 pt"** (`format!("Back to {:.0} pt", DEFAULT_MARGIN)`).
  - Hidden while |value − 34| < 0.5 (S:653-654). A click turns it "on"; the next poll (up to 0.5 s later) sets the slider to 34 and turns it off (S:649-652).
  - **No double-click reset** exists.
- **Sounds switch**: on the Audio tab, not here ("Start and stop sounds" → `sounds`).
- **Accessibility note**: shown in the footer when the window opens (exact text in §3). It is the only permission note and is not repeated in the Dictation tab body.

---

## 7. Every user-visible string in settings.rs and spotgrid.rs

settings.rs:
- Tab names: `General`, `Audio`, `Dictation`, `Dictionary`
- Group headers: `Soniox`, `Startup`, `Input`, `Shortcut`, `Window`, `Words to expect`
- Row labels: `API key`, `Languages`, `Launch at login`, `Microphone`, `Start and stop sounds`, `Key`, `Mode`, `Follow pointer`, `Position on screen`, `Distance from the edge`
- Placeholder: `Paste your key`
- Mode options: `Hold to talk`, `Click on, click off`
- Microphone items: `System default ({name})`, `System default`, `{chosen} (not connected)`
- Reset button: accessibility `Reset`, tooltip `Back to 34 pt`
- Hints:
  - `Get a key at console.soniox.com — it is stored on this Mac only. Languages are the ones that may turn up in your audio, as names or codes: English, uk, ru.`
  - `Voi starts in the menu bar when you log in, so the dictation key works from the first minute. The same switch is in System Settings ▸ General ▸ Login Items.`
  - `Used for dictation and for your side of a call. AirPods switch from music to headset mode whenever their microphone opens, which interrupts what is playing and takes a second or two — choose the MacBook's microphone to keep them for listening only. Dictation sounds tell you it heard the key while you look at what you are typing into.`
  - `Speak while it listens; it is typed where the cursor is when it stops. Let go with the pointer on the window to throw it away instead.`
  - `Above the pointer and to its right; left of it or under it where the screen runs out. Never on it — the pointer on the window throws a dictation away.`
  - `Names, jargon and product names that come up in your calls. Commas or new lines between them. A hint, not a filter: nothing is forced to match.`
- Counter: `{n} term{s} · {used} of about 10000 characters`
- Accessibility note: `Voi needs Accessibility to type for you — System Settings ▸ Privacy & Security ▸ Accessibility. Until then dictation copies to the clipboard.`
- Otherwise the footer shows the `settings.json` path
- Status messages:
  - `Could not save the key: {e}`
  - `Key saved.`
  - `The dictionary is {count} characters; Soniox takes about 10000. Not saved.`
  - `Could not save: {e}`
  - `Voi will launch at login.`
  - `Voi will not launch at login.`
  - `Allow Voi in System Settings ▸ General ▸ Login Items to finish turning on launch at login.`
  - `This Mac has no Login Items service.`
  - `Could not change launch at login: {e}.`
  - `Saved: {languages}, {n} term{s}, {mic}, {key} {placement}, {margin}pt in, sound {on|off}.` with the fragments `any language`, `default microphone`, `beside the pointer`, `on`, `off`
- From other files but shown in the window:
  - Key labels (P:74-79)
  - Spot labels, lowercased, inside the receipt (P:132-140)
  - `version_line` formats (M:621-624)
  - login.rs error strings (L:72, 74)

spotgrid.rs has no user-visible strings: no labels, tooltips or accessibility text.

---

## 8. Hard to reproduce on Windows

- The `NSTabViewController` Toolbar tabs combined with the `Preference` toolbar style: icon-over-label buttons centred in a unified titlebar/toolbar, the selected one highlighted, and the window title following the selected tab.
- SF Symbols: `gearshape`, `mic`, `command`, `character.book.closed`, `arrow.counterclockwise`. Windows has no direct equivalents; they need substitute glyphs or icons.
- `NSSwitch`, whose size comes only from `fittingSize()` with no number in the code. Also the native looks of `NSPopUpButton`, `NSTokenField` (token chips, comma ends a token) and `NSSecureTextField`.
- Semantic, appearance-adaptive colours: `quinarySystemFillColor`, `separatorColor`, `secondaryLabelColor`, `tertiaryLabelColor`, `controlAccentColor`, `systemRedColor`, and the default window background, with automatic light/dark. The spot grid's sRGB values are fixed and do not change with the theme.
- `monospacedDigitSystemFont`; the system font (SF Pro) at 13/11/10 pt with semibold headers. Measured hint heights (28 pt per two lines, 56 per four) depend on SF metrics, so the Dictation tab's 16 pt margin can overflow with other fonts.
- Saving the key "only when focus has left the field" relies on `currentEditor()`.
- Launch at login through `SMAppService`, including the `NeedsApproval` state and the "System Settings ▸ General ▸ Login Items" wording.
- The Accessibility permission (`AXIsProcessTrusted`) and its "System Settings ▸ Privacy & Security ▸ Accessibility" note have no Windows counterpart.
- Live microphone list refresh comes from Core Audio property listeners. On non-macOS, non-Windows builds the stub returns `false`; the Windows implementation lives in `watch.rs`, not read here.
- The Mac-only modifier keys and glyphs (⌘ ⌥ ⌃, Fn/Globe, left/right detection by device-dependent flag bits).
- An Accessory app having to activate itself to bring the window to the front; the ⌘, menu shortcut.
- The coordinate system: AppKit's y counts upward. All positions above are already converted to top-down.
## Voi macOS spec for the Windows port: dictation, menu bar, startup, files

Paths below are relative to the repository root. Abbreviations used:
- `voice` = `apps/voi-app/src/mac/voice.rs`
- `mod` = `apps/voi-app/src/mac/mod.rs`
- `hotkey` = `apps/voi-app/src/mac/hotkey.rs`
- `paste` = `apps/voi-app/src/mac/paste.rs`
- `sound` = `apps/voi-app/src/mac/sound.rs`
- `meter` = `apps/voi-app/src/mac/meter.rs`
- `sweep` = `apps/voi-app/src/mac/sweep.rs`
- `state` = `apps/voi-app/src/state.rs`
- `engine` = `apps/voi-app/src/engine.rs`
- `dict` = `crates/voi-core/src/dictation.rs`
- `prefs` = `crates/voi-core/src/prefs.rs`
- `perm` = `crates/voi-audio/src/permissions.rs`

Coordinates are AppKit's: y points up and the origin is at the bottom-left.

---

## A. Dictation

### A1. The key

- **Options** (prefs:52-97). `DictationKey`, serialized in kebab-case:

  | Variant | Label | Mask |
  |---|---|---|
  | `RightCommand` (**default**) | `"Right ⌘"` | `0x0000_0010` |
  | `LeftCommand` | `"Left ⌘"` | `0x0000_0008` |
  | `RightOption` | `"Right ⌥"` | `0x0000_0040` |
  | `LeftOption` | `"Left ⌥"` | `0x0000_0020` |
  | `RightControl` | `"Right ⌃"` | `0x0000_2000` |
  | `Fn` | `"Fn (Globe)"` | `0x0080_0000` |

  The masks are the device-dependent modifier bits (left and right sides) plus Apple's Function flag.
- **How it is read** (hotkey:61-68). The key is watched, not intercepted. The keystroke still reaches the frontmost app.
  - `Hotkey::pressed`: the key counts as down if `NSEvent::modifierFlags_class() & mask != 0`, OR `CGEventSource::flags_state(CombinedSessionState) & mask != 0`.
  - No event tap is used, so it needs neither Accessibility nor Input Monitoring.
- **Polling.** The key is polled every frame from the 60 fps `frame:` timer (mod:208-244 → `dictate()` at mod:425). The timer is in `NSRunLoopCommonModes` (mod:825-832), so it keeps firing while a menu is open.
- **Combos are not detected.** The code does not check whether another key is held with it. A press of Right ⌘ inside a combo (for example Right ⌘+C) counts as a press. The comment at hotkey:83-85 accepts that "a brush costs a flash and nothing typed".
- **First press is logged** once per run: `"dictation key seen"` (mod:430-432).
- **Mode** (prefs:25-33). `DictationMode` is `push-to-talk` (**default**) or `toggle`.
- **Windows-only fields already in Prefs.** `record_hotkey` defaults to none. The Mac ignores both.

### A2. Hold vs toggle logic (hotkey:72-114)

- `CLICK = 450 ms` (hotkey:30).
- `running` is `voice.listening()`, which is `session.is_some()` (voice:605). It stays true through the post-release flush until the text is delivered, cancelled or failed.

Each case of `poll`:

- **Key goes down** (`since == None`):
  - Records `since = now`.
  - `armed = (mode == PushToTalk) && !running`.
  - In push-to-talk this returns **Start on the very frame the key goes down**. There is no delay.
- **Key still down:** nothing.
- **Key released:**
  - Push-to-talk: returns **Stop** only if `running && armed`, meaning this hold started the dictation. A press while already running neither starts nor stops anything.
  - Toggle: returns Start (not running) or Stop (running) **only if the hold lasted under 450 ms**. Longer holds do nothing. Toggle never starts on key-down.

### A3. What starts, stops and cancels

**Start** (mod:443-466):
1. Calls `find_api_key()`.
   - On failure it sets `shared.error = e.to_string()` and logs `"dictation needs a key"`. No window and no sound.
2. Logs `"dictation starting"`, enters the tokio runtime, and calls `voice.start(...)`.
3. **Then plays the Start sound** (gated by `prefs.sounds`). This happens immediately, before the microphone has opened.

`Voice::start` (voice:610-640):
- Returns early if a session already exists.
- Clears settled, tail, note and the per-character timestamps, and sets `listening = true`.
- Resets the timers.
- Captures `sounds = prefs.sounds`.
- Calls `Dictation::start(api_key, languages, terms, microphone)`.
- Calls `place()`, sets `wanted = 1.0`, then `orderFrontRegardless()`.
- `open`, the meter and `hover` are **not** reset. A window still leaving continues from its current `open` value.

**Stop** (mod:468-483):
- If `voice.hovered()` (pointer on the glass and a session exists): runs `voice.cancel()`.
- Otherwise:
  1. Logs `"dictation stopping"`.
  2. Calls `voice.finish()`.
  3. **Plays the End sound right away, on release.** It does not wait for the text.

`Voice::finish` (voi:643-669):
- Calls `Dictation::finish()`, which sends stop. The mic task ends and dropping the audio sender makes Soniox flush.
- Re-stamps every not-yet-shown character birth and settle time to `now, now+5ms, now+10ms…` (`FLUSH = 5 ms`, voice:67).
- Sets `finishing = released = now` and `listening = false`.

**Cancel** (voice:721-737):
- `session.take()` drops the `Dictation`, which closes the microphone.
- Clears `finishing` and `confirmed`, sets `listening = false` and `wanted = 0.0`. The window leaves immediately with no pause.
- Plays the **End** sound, using the `sounds` value captured at start.
- Logs `"dictation cancelled"`.
- Nothing goes on the clipboard and nothing is typed.

**Click-to-cancel** (voice:799-804):
- Each frame it polls `NSEvent::pressedMouseButtons() & 1`.
- On the press edge (down now, not down last frame) while `hovered()`, it calls `cancel()`.
- The panel has `ignoresMouseEvents(true)` (voice:548), so **the click also passes through** to whatever window is underneath.

**Hover test** (voice:687-708):
- Uses screen mouse location against the glass rect only, not the padded window.
- Uses the live slide offset, except in follow-pointer mode, where it tests the glass at home (offset 0).
- Returns false when the panel is not visible.

### A4. Sounds (sound.rs)

- **Files:** `start.mp3` and `end.mp3`. They are loaded from the main bundle's Resources via `pathForResource("start"/"end", ofType: "mp3")` (sound:28-33, 68-73).
  - Source copies are `assets/sounds/start.mp3` and `assets/sounds/end.mp3`, copied into `Contents/Resources/` by `scripts/bundle.sh:79`.
  - Both are mono, 44.1 kHz MP3, about 0.914 s long (measured with `afinfo`).
- **Loading:** loaded once, lazily, on the first play, and cached in a main-thread `thread_local` (sound:36-41, 53).
  - If the file is missing: logs `"no sound in the bundle"` and stays silent.
  - If it fails to load: logs `"sound would not load"`.
- **Playing:** `sound.stop()` then `sound.play()`, so a quick repeat press restarts the note (sound:59-63).
- **Volume:** no volume is set anywhere, so it plays at the `NSSound` default of 1.0.
- **Gate:** sounds play only when `prefs.sounds` is true (default `true`, prefs:312).
- **When each plays:**
  - Start: right after `voice.start()` on the Start trigger.
  - End: on the Stop trigger, whether it finishes or cancels; also on click-cancel.
  - **No** sound on failure, on delivery, or when the flush deadline expires.

### A5. The glass window: geometry and look

**Constants (voice:44-52, 76):**

| Constant | Value |
|---|---|
| `GLASS` | 368.0 × 93.0 |
| `PAD` | `sweep::REACH + 5.0` = 20 + 5 = **25** |
| `TRAVEL` | 30.0 |
| `CORNER` | 13.0 |
| `MARGIN` (text side inset) | 18.0 |
| `TEXT_SIZE` | 13.0 |
| `METER_H` | 22.0 |

**Window size** (voice:522-525): `GLASS + 2*(PAD+TRAVEL)` = **478 × 203**. The glass sits centred in it, 55 pt from each edge, plus the live offset (`glass_rect`, voice:459-467).

**Panel** (voice:527-548):
- `NSPanel`, Borderless | NonactivatingPanel, buffered.
- Level `25` (NSStatusWindowLevel).
- Not opaque, clear background, `hasShadow = false`.
- Collection behaviour: `CanJoinAllSpaces | FullScreenAuxiliary`.
- `hidesOnDeactivate(false)`, `becomesKeyOnlyIfNeeded(true)`, `ignoresMouseEvents(true)`.
- Never takes focus.

**Backdrop** (voice:554-568):
- `NSVisualEffectView` the size of the glass.
- Material `Popover`, blending `BehindWindow`, state `Active`, appearance `NSAppearanceNameVibrantDark`.
- Layer `cornerRadius = 13`, `masksToBounds`.
- Its frame moves with the slide offset (voice:959).

**Border** (voice:176-183): rounded rect inset 0.5, radius 13, width 1.0, stroke sRGB(1,1,1, **0.10 × open**).

**Meter** (voice:241-254, `meter::paint`):
- Runs edge to edge across the glass (x from `glass.x` to `glass.x+368`).
- Centred at `y = glass.y + METER_H/2 + 2` = glass.y + 13. Max bar height `18.0`.
- `fade = open`.
- `tint = 1` while listening (red), `0` otherwise (white). "Listening" is the shared flag, which turns false the moment the key is released.
- Colour (meter:22-30): mix of white and `RED = (1.0, 0.25, 0.22)` (meter:15) by tint. One colour for all bars, alpha = fade.
- Bar layout (meter:83-115) for width 368: `room = clamp((368-45)/200, 0, 1) = 1` gives pitch 2.1 and fill 0.62.
  - `slots = (368/2.1) as usize | 1` = 175 bars.
  - `step = 368/175`; `bar_w = clamp(step*0.62, 0.9, 2.4)` ≈ 1.30.
  - Each bar averages a window of `span*1.6` of the meter's 121-sample wave (`SPREAD = 60`, state:30).
- Bar height (meter:128-129): `max(18 × level × ends_taper(r) × fade, 1.0)`. Corner radius is `min(bar_w, h)/2`. All bars are filled as one path.
- `ends_taper` (meter:45-58): 1.0 up to `SHOULDER 0.55`, then smoothstep down to `ENDS 0.12`.
- Meter dynamics (state:96-176):
  - Constants: `ATTACK_TAU 0.055`, `DECAY_TAU 0.09`, `RANGE_TAU 3.0`, `FLOOR 0.34`, `CONTRAST 1.7`, `GATE 0.26`, `LEAK 0.11`, history at `HISTORY_HZ 120`.
  - It is a delay line radiating out from the centre. Floor per bar is `0.004`.
  - The dictation window has its own `Meter` instance, ticked with `dt = 1/60` (voice:861, 889-890).
- Meter input (voice:888):
  - While listening: the latest `DictationEvent::Level`.
  - After release: a "parting" swell, `(1 - elapsed/PARTING).clamp(0,1)^1.6` with `PARTING = 1500 ms` (voice:80, 876-879).
  - `meter_target` is not reset between dictations.

### A6. Text rendering

**Font:**
- `NSFont.monospacedSystemFont(13, weight 0.0)`, i.e. SF Mono Regular (overlay.rs:1799-1803, used at voice:418).
- Line height = `NSLayoutManager.defaultLineHeight(font)`, cached (voice:57-63). Measured on this Mac: **16.0 pt**.
- Paragraph: `lineBreakMode = ByWordWrapping`, left aligned (voice:408-409).

**Colour** (voice:400-407):
- `mix(unsure, sure)` by `certain` (0..1).
- Unsure (grey, provisional): sRGB **(0.62, 0.66, 0.72)**.
- Sure (settled): sRGB **(0.96, 0.97, 0.99)**.
- Alpha = `open × showing`.

**What is shown** (voice:192-197):
- If a `note` is set, it is shown at `alpha = open` and `certain = 1.0` (all white, no reveal).
- Otherwise `revealed(settled, tail, …)` is shown.

**Per-character reveal** (voice:321-396):
- Each character has a `born` time and, for settled characters, a `settled_at` time.
- `showing = clamp((now - born)/ARRIVING)` with `ARRIVING = 50 ms` (voice:322).
- `certain = clamp((now - settled_at)/WHITENING_TIME)` with `WHITENING_TIME = 50 ms` (voice:324). Tail characters have no `settled_at`, so they stay grey.
- Rendering **stops at the first character with `showing <= 0`**.
- Both values are rounded to 1/40 (`STEPS = 40`, voice:358), and runs of equal values are merged.

**Stamping births** (voice:907-935, every frame):
- New characters get `born = max(last + gap, now)`, then step by `gap`.
- `gap = min(LETTER, TRAIL / backlog)` with `LETTER = 3 ms` (voice:330) and `TRAIL = 50 ms` (voice:341).
- New settled characters get `settled_at` the same way with `gap = min(WHITENING = 2 ms` (voice:333), `TRAIL/backlog)`.
- Births are never in the past. The arrays only grow: a tail rewritten shorter keeps the birth times by index.

**On release:** pending stamps are compressed to `now + k×FLUSH` with `FLUSH = 5 ms` (see A3).

**Layout and clipping** (voice:198-237):
- Text width = 368 − 2×18 = **332**.
- `bottom = glass.y + METER_H + 6` (glass.y + 28).
- `room = glass.top − 12 − bottom` = **53**.
- `top = bottom + max(floor(room/line), 1) × line`. At 16 pt that is **3 lines = 48 pt**, i.e. glass.y+28 to glass.y+76.
- The clip is that rect. Text is drawn in a rect whose **bottom** sits on `bottom` and whose height is `max(needed, window height)`.
- Result: the newest line stays visible and older lines scroll up and out. The cut always falls between whole lines (terminal style).
- Nothing is drawn if the shown string is whitespace-only.

### A7. Placement (voice:742-788, prefs)

1. **Screen.** Uses the screen whose `frame` contains `NSEvent.mouseLocation`, falling back to `mainScreen`. Placement uses that screen's **`visibleFrame`**, which excludes the menu bar and the Dock.
2. **Window origin** = `glass origin − (PAD+TRAVEL)` = glass origin − 55. The panel is placed once per start with `setFrameOrigin`. It does not move while it is open.

**Grid mode** (default):
- `prefs.dictation_spot.place(origin, size, (368,93), prefs.dictation_margin)` (prefs:169-191).
- `Spot` default is `BottomCenter`. `dictation_margin` default is `DEFAULT_MARGIN = 34.0` (prefs:299).
- x:
  - Left spots: `left + margin`.
  - Centre column: `left + (width − w)/2` (the margin is ignored).
  - Right spots: `left + width − w − margin`.
- y:
  - Top spots: `bottom + height − h − margin`.
  - Middle row: `bottom + (height − h)/2`.
  - Bottom spots: `bottom + margin`.
- Travel direction `from = spot.slide()` (prefs:152-160):
  - Top row: `(0, +1)`.
  - Bottom row: `(0, −1)`.
  - MiddleLeft: `(−1, 0)`.
  - MiddleRight: `(+1, 0)`.
  - Center: `(0, 0)`, fade only.

**Follow-pointer mode** (`dictation_follow_pointer`, default false), using `beside_pointer` (prefs:208-228):
- Constants: `BESIDE_POINTER = 10.0`, `UNDER_POINTER = 28.0` (prefs:195-197).
- `x = px + 10`. If `x + w > right`, then `x = px − 10 − w`.
- `y = py + 10`. If `y + h > top`, then `y = py − 28 − h`.
- Then clamp to `[left, right−w]` and `[bottom, top−h]`.
- `facing_corner` (prefs:233-244):
  - Corner x = `x` if `px <= x + w/2`, else `x + w`.
  - Corner y = `y` if `py <= y + h/2`, else `y + h`.
  - Returns `(dx, dy) = pointer − corner`.
- `from = (clamp(dx/30, −1, 1), clamp(dy/30, −1, 1))` (voice:770). The glass starts with that corner at the pointer tip and shoots out to its spot. It is moved, never scaled.

### A8. Arrive and leave animation (voice:838-962)

**Spring** (semi-implicit Euler, fixed `dt = 1/60`):
- `speed += ((wanted − open)·k − speed·c)·dt`, then `open += speed·dt`.
- Grid: `k = 420.0`, `c = 26.0`. Beside the pointer: `k = 1100.0`, `c = 52.0`.
- Snaps to `wanted` when `|open − wanted| < 0.002` and `|speed| < 0.05`.
- `orderOut` when `open <= 0.001 && wanted == 0`.

**Slide:** `offset = from × (1 − open) × TRAVEL`. It goes slightly negative on overshoot, which gives the bounce. The glass moves inside a fixed window.

**Alpha:**
- `panel.alphaValue = clamp(open / 0.35, 0, 1)`. Fully opaque a third of the way in.
- This and the blur and sweep shifts are updated only when `|open − before| > 0.0005`.
- Drawing uses `s.open = clamp(open, 0, 1)`. `paint` returns immediately if `open <= 0.01`.

### A9. Hover-to-cancel veil (voice:261-317, 882-886)

- **Ramp:** `hover` moves linearly toward `wanted = (over && session.is_some()) ? 1 : 0` at a step of `(1/60)/0.10`, i.e. **0.10 s** up and 0.10 s down. `shade = hover × open`.
- **Drawn over everything, in this order:**
  1. Glass-shaped rounded rect (radius 13) filled with black sRGB(0, 0, 0, **0.40 × shade**).
  2. Then filled with RED sRGB(1.0, 0.25, 0.22, **0.20 × shade**).
  3. Ring: inset 0.75, radius 13, width **1.5**, stroke RED at **0.75 × shade**.
  4. Label **`"Cancel"`** in `NSFont.systemFont(13)` (proportional, not mono), centred, colour sRGB(**1.0, 0.44, 0.40**, shade), vertically centred in the glass.
- **Availability:** cancel is possible from start until `complete`. That includes the flush window after release.

### A10. Release, flush and delivery

**Deadline** (dict:23-46):
- `FLUSH_GRACE = 1800 ms`, `GIVE_UP = 10 s`.
- `flush_deadline(began, released, connected)`:
  - `cap = released + 10 s`.
  - If not connected: deadline = `cap`.
  - Otherwise: `caught_up = connected + (connected − began)`, and deadline = `min(max(released, caught_up) + 1.8 s, cap)`.

**Each frame after release** (voice:809-829):
- If `Done(text)` arrives first, it calls `complete(text)`.
- Otherwise, once past the deadline, it logs:
  - `"dictation did not finish in time; using what arrived"` (if connected), or
  - `"dictation never reached soniox; what was said is lost"` (if not),
  - then calls `complete(assembled(settled, tail))`.

**`assembled`** (dict:218-231): concatenates settled and tail, collapses runs of space, `\n` or `\t` into a single space, and trims.

**`complete`** (voice:1014-1035):
- Drops the session and sets `listening = false`.
- Empty or whitespace text: `note = ""`, stays `DELIVERED = 0`.
- Otherwise it calls `deliver(text)`:
  - `Pasted`: `note = text`, shown all white, `DELIVERED = Duration::ZERO` (voice:84). The window starts leaving on the same frame.
  - `Copied`: `note = "Copied — press ⌘V to paste.\n\n{text}"`, stays `TO_READ = 2500 ms` (voice:87).
- After `stay` has elapsed, `wanted = 0` (voice:831-836).

**`deliver`** (paste:77-93):
- Always: `NSPasteboard.general.clearContents()`, then `setString(text, .string)`. The user's clipboard is overwritten and not restored.
- `AXIsProcessTrusted()`:
  - If false: calls `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`, which shows the system Accessibility dialog (macOS shows it once), and returns `Copied`.
  - If true: synthesizes ⌘V (paste:96-110). `CGEventSource(.Private)`, keyboard event keycode **`9`** (V, physical key, `KEY_V`, paste:23), key-down then key-up, each with `CGEventFlags::MaskCommand`, posted to `HIDEventTap`. Returns `Pasted`.
- Accessibility is requested **at first delivery, never at launch**.

**Failure** (voice:998-1007):
- Triggered by `DictationEvent::Failed(msg)`, which only happens when the microphone will not open.
- Drops the session, `listening = false`, `note = msg`, stays `TO_READ` 2.5 s. No sound.
- The message is `format!("{e:#}")` of the context chain (dict:105-108), e.g. `"could not open the microphone: no default input device — check microphone permission"`.
- Other possible causes (crates/voi-audio/src/mic.rs):
  - `"failed to read default input config"`
  - `"failed to spawn microphone thread"`
  - `"microphone thread died during startup"`
  - `"unsupported microphone sample format: {other:?}"`
- Warnings are only logged as `"dictation"`, never shown:
  - `DictationEvent::Warning` (`"reconnecting (attempt {attempt})"`, dict:180-182)
  - Soniox error text

**Stream details** (dict:79-198):
- Soniox config: `TARGET_RATE`, 1 channel, `language_hints = prefs.languages` (if non-empty; default `["uk","ru","en"]`), `context_terms = prefs.terms`, `enable_speaker_diarization = false`.
- The microphone opens off-thread via `MicSource::start_named(prefs.microphone)` and falls back to the default device.
- `Final` tokens append to `settled` and clear `tail`. `Pending` **replaces** `tail`.
- Logs `"first words"` once, with `after_key_ms` and `after_connect_ms` (voice:979-986).

### A11. Edge sweep glow (sweep.rs, voice:938-961)

- **Lit while `listening()`**, i.e. `session.is_some()`. It stays red **through the post-release flush** until complete, cancel or fail. The meter turns white at release, but the sweep does not go out until then.
- **Fade:** exponential with τ = **0.35 s** (sweep:260), snapping when within 0.004.
- **Two passes:**
  - Glow, drawn first: opacity `alpha × 0.30` (`GLOW_ALPHA`).
  - Line, drawn on top: opacity `alpha × 1.0`.
- **Blob:** a radial gradient in sRGB **(1.0, 0.30, 0.24)**.
  - Alpha stops `[(0,1.00),(0.25,0.74),(0.50,0.32),(0.75,0.07),(1,0)]` (sweep:49-55).
  - Diameter = `perimeter × SPAN (0.60)`.
- **Movement:** keyframe-animated along the rounded-rect track (glass inset 0.5, radius 13) at paced constant speed, repeating forever.
  - Lap = `3.4 × max(perimeter/reference, 1)^0.35` s. The reference is the glass's own perimeter, so the lap is **3.4 s**.
  - Phase carries across animation rebuilds.
- **Masks:**
  - Line mask: track stroked at width `CORE = 1.4`.
  - Glow mask: `STEPS = 56` strokes. For `r` from 20 down to 0 (`REACH = 20`), width `2r + 1.4`, alpha `0.17 × exp(−(r/4)²)` (`FALLOFF = 4`).
- **Layering:** the layers hang on the view's layer, above the blur. They translate with the glass offset (`shift`).

### A12. Dictation during a recording

**Yes.** The dictation window is independent of the recorder. It opens a second stream of the same microphone (dict:1-8, voice:1-7, mod:53-56). Its own capture is excluded from the "someone else is listening" watch, which filters by PID. The menu and recording state are unaffected. Dictation writes nothing to disk.

---

## B. Menu bar

### B1. Status item (mod:265-266, 382-390, 514-529, 562-608)

- `NSStatusBar.system.statusItem(withLength: NSVariableStatusItemLength)`.
- **Title:** always `""` (`clock_title()`, state:502-504).
- **Image:** 18×18 (`SIZE`), dot radius **4.4**, centred.
- **Idle** (`mode == Idle`):
  - Colour (1, 1, 1), alpha `dot_alpha()` = 1.0.
  - `image.isTemplate = true`, so macOS renders it in the menu bar's foreground colour (white on dark, black on light).
  - Rebuilt only when `dot_alpha` changes.
- **Active** (Recording or Stopping):
  - Colour sRGB **(0.93, 0.24, 0.20)**, `isTemplate = false`.
  - Alpha 1.0 when Recording, **0.85** when Stopping (state:511-520).
  - Redrawn every 4th frame (about 15 Hz) with a halo.
- **Halo:**
  - `phase = 0.5 − 0.5·cos(2π·pulse)`. `pulse` advances at `1/PULSE_PERIOD` with `PULSE_PERIOD = 3.6 s` (state:87, 181), ticked in `frame:`.
  - 8 rings, drawn from the outermost in. For `t = i/7`:
    - Radius `4.4 + 0.8 + t·(2.2 + 2.6·phase)`.
    - Alpha `0.20·(1−t)^1.5·(0.35 + 0.65·phase)`.
    - Same RGB as the dot.

### B2. Menu, in order (mod:303-360)

`autoenablesItems = false`. No item ever sets a checkmark (`state`). Key equivalents are a lowercase letter, so they are ⌘+key, and they only work while the menu is open (the app is Accessory, with no main menu).

| # | Title | Key | Action | Enabled |
|---|---|---|---|---|
| 1 | Header: `decorate(summary(), spin)` (initial `"Idle"`) | – | none | always **disabled** |
| – | separator | | | |
| 2 | `toggle_title()`: `"Start Recording"` / `"Stop Recording"` / `"Stopping…"` | ⌘R `"r"` | `engine::toggle` (Idle→start, Recording→stop, Stopping→nothing) | yes |
| 3 | `"Transcribe File…"` | ⌘T `"t"` | NSOpenPanel (single file, no folders, message `"Choose a recording to transcribe"`), activates the app, transcribes, then reveals the result in Finder | yes |
| 4 | `"Open Last Transcript"` | – | opens the newest `sessions/*.md` | only if `latest_transcript()` exists; re-checked on every refresh (mod:392-400) |
| 5 | `"Open Sessions Folder"` | – | creates and opens `sessions/` | yes |
| 6 | Initially `"Show Overlay"`, then `"Keep Overlay Visible"` (unpinned) or `"Unpin Overlay"` (pinned) | ⌘O `"o"` | `toggleOverlay:` flips `overlay_pinned`; **not persisted** | `!hidden` |
| 7 | `"Hide Voi"` / `"Show Voi"` (when hidden) | ⌘H `"h"` | flips `hidden` and **saves it to settings.json** | yes |
| – | separator | | | |
| 8 | `"Settings…"` | ⌘, `","` | opens the Settings window | yes |
| 9 | `"Audio Permissions…"` | – | opens `x-apple.systempreferences:com.apple.preference.security?Privacy_AudioCapture` | yes |
| – | separator | | | |
| 10 | `"Quit Voi"` | ⌘Q `"q"` | see C5 | yes |

### B3. Header and summary texts

- **`summary()`** (state:476-487): if a file job is running, its text. Otherwise:
  - `"Idle"`
  - `"Recording"`
  - `"Stopping — saving transcript…"`
- **Job texts** (engine):
  - First `"Uploading {file name}…"` (engine:185).
  - Then `progress_text` (engine:218-231): `"Uploading"`, `"Queued"`, `"Transcribing {h:mm:ss} of audio"`, `"Transcribing"`, `"Saving"`.
- **`decorate`** (mod:648-654):
  - `"Idle"` and `"Recording"` are shown as-is.
  - Anything else becomes `"{FRAME}  {summary}…"` (two spaces, then a trailing `…`).
  - `FRAMES = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧"]`.
  - Faithful-copy quirks: the result reads `"Stopping — saving transcript……"` and `"Uploading x.m4a……"` (double ellipsis).
- **Spinner timing:** `spin` advances and the menu refreshes every 8 frames (about 7.5 Hz) **only while `job` is Some** (mod:240-243). In Stopping without a job, the frame is static.
- **Refresh timers:** a 1 s `tick:` timer and a 60 fps `frame:` timer, both in common modes, so the menu updates while it is open.

### B4. Error display

`State.error` is set in many places but **no Mac UI code reads it**. It is not in the menu, the overlay or Settings; grep finds only writes. Errors are effectively visible only in the log. The strings, as written:
- `"Microphone: {e}"` / `"System audio: {e}"` (preflight, mod:801-804)
- `"{origin}: {detail}"` / `"{origin}: {message}"`, where origin is `"You"` or `"Them"` (engine:145, 149)
- The `find_api_key` error: `"no Soniox API key — paste it into Settings, set SONIOX_API_KEY, or write it to {path}"` (recorder.rs:792-795)
- Recorder or transcription errors `{e:#}`

---

## C. Startup, permissions, errors

### C1. Command-line flags (mod:698-732)

Only the first argument is checked. Both flags run before logging and UI.

- **`--check-settings`:** builds Settings off-screen and prints `"{tab}: {room:.0} pt to spare"` per tab. Exits 0 if all are ≥ 0, else 1.
- **`--launch-at-login on|off|status`** (no argument means `status`):
  - Prints `{Status:?}`: `Off`, `On`, `NeedsApproval` or `Unavailable`.
  - On error, prints to stderr and exits 1. Errors:
    - `"expected on, off or status, not {other:?}"`
    - `"only the installed Voi.app can launch at login, not a bare binary"` (when the exe path does not contain `.app/Contents/MacOS/`)
    - `"this Mac has no Login Items service"`
    - The `NSError` `localizedDescription`
  - Uses `SMAppService.mainAppService` (login.rs). Nothing is stored in settings.

### C2. Launch sequence

1. `init_logging()`, then log `"voi app starting"` with `build = version_line()`.
   - `version_line()` is `"Voi {ver} · build {build} · {commit}"` or `"Voi {ver} (unpackaged)"` (mod:616-625). It is only logged, not shown.
2. `NSApplication` activation policy **Accessory**: no Dock icon. The Info.plist also has `LSUIElement = true`.
3. Load prefs and log `"settings loaded"` with languages, term count and hidden. Copy `hidden` into state.
4. **Mic-in-use watch thread** (mod:762-785), polling every **1200 ms**:
   - `mic_in_use = someone_else_is_listening()` (any other PID running input). It drives the overlay's "Record?" prompt and visibility.
   - Orphan check: if `open_streams() == 0 && we_are_listening()` for **3** consecutive polls, logs the error `"microphone still open with no capture running — a stream was dropped without closing its device"`.
5. Build the controller (status item and menu).
6. **Permission preflight thread** (mod:792-805, perm:34-56):
   - `MicSource::start()`, hold **250 ms**, drop.
   - `SystemSource::start()`, hold **250 ms**, drop.
   - This deliberately triggers the TCC prompts at launch.
   - Failures log `"microphone unavailable"` / `"system audio unavailable"` and set `error` (not displayed, see B4).
   - A denied system-audio tap does not fail here; it just delivers silence.
   - The system dialog texts come from Info.plist (scripts/bundle.sh:62-65):
     - `NSMicrophoneUsageDescription` = `"Voi transcribes your side of a call."`
     - `NSAudioCaptureUsageDescription` = `"Voi transcribes what the other participants say."`
7. Timers start: `tick:` every 1.0 s, `frame:` every 1/60 s. Then `app.run()`.

### C3. Accessibility and Input Monitoring

- Accessibility is requested only on the first delivery when it is missing (A10).
- The dictation key needs no permission. Input Monitoring is never needed.

### C4. Logging (mod:674-692)

- Writes to `$HOME/Library/Logs/Voi.log`. The directory is created; the file is opened in append mode.
- Filter: `RUST_LOG`, else `"warn,voi=info,Voi=info"`. No ANSI.

### C5. Quitting while recording (mod:247-259, 376-379)

- If a recording is active, it calls `engine::stop` and sets `quitting = true`. The next `refresh()` (1 s tick) calls `terminate()` once the mode is back to Idle, after the transcript is flushed.
- Otherwise it terminates immediately.
- A dictation in progress is not considered. It dies with the process.

### C6. Settings autosave

- Every 30 frames (0.5 s), while the Settings panel exists (mod:232-234, 494-511).
- If it saved something, it reports the message, reloads prefs into the cached copy the frame loop reads, and re-syncs `hidden`.

---

## D. Files written

The data directory is `dirs::data_dir()/voi`, which on macOS is `~/Library/Application Support/voi`. The `VOI_DATA_DIR` environment variable overrides it (crates/voi-core/src/lib.rs:183-189). On Windows the same code gives `%APPDATA%\voi`.

| File | Contents and when |
|---|---|
| `voi/sessions/{YYYYMMDD-HHMMSS}-{uuid first 8}.md` | One per recording or file transcription (lib.rs:92-117). Header `"# {local YYYY-MM-DD HH:MM}\n\n"`. Lines `"[h:mm:ss] {Speaker}: {text}"`, with a blank line when the speaker changes. Flushed per segment. |
| `voi/sessions/{id}-mic.wav`, `{id}-system.wav` | Only when audio capture is asked for (lib.rs:120-127). |
| `voi/settings.json` | Pretty JSON of `Prefs` (prefs:350-379). Keys: `languages`, `terms`, `hidden`, `dictation_mode` (`"push-to-talk"`/`"toggle"`), `dictation_spot` (kebab-case, e.g. `"bottom-center"`), `dictation_follow_pointer`, `dictation_key` (`"right-command"`, `"left-command"`, `"right-option"`, `"left-option"`, `"right-control"`, `"fn"`), `dictation_margin`, `sounds`, `microphone`, `record_hotkey`. Written by Settings autosave and by Hide/Show Voi. A missing or damaged file falls back to defaults. |
| `voi/config.env` | `"SONIOX_API_KEY={key}\n"`, chmod 600 (owner-only ACL on Windows) (recorder.rs:719-743). Key lookup order: `SONIOX_API_KEY` env, then this file, then `./.env`. |
| `voi/overlay-position` | `"{x} {y}"` (recorder.rs:746-764). Written by the overlay when dragged. |
| `~/Library/Logs/Voi.log` | Appended tracing log. |
| Clipboard | Dictation overwrites the general pasteboard with the text. Copy Transcript also writes it. |

Dictation itself writes nothing to disk.

---

## E. User-visible strings (these files)

**Dictation:**
- `"Cancel"`
- `"Copied — press ⌘V to paste.\n\n{text}"`
- `"could not open the microphone: {cause}"`, with the cause chain from mic.rs:
  - `"no default input device — check microphone permission"`
  - `"failed to read default input config"`
  - `"failed to spawn microphone thread"`
  - `"microphone thread died during startup"`
  - `"unsupported microphone sample format: {fmt}"`
- Key labels: `"Right ⌘"`, `"Left ⌘"`, `"Right ⌥"`, `"Left ⌥"`, `"Right ⌃"`, `"Fn (Globe)"`
- Spot labels: `"Top left"`, `"Top centre"`, `"Top right"`, `"Middle left"`, `"Centre"`, `"Middle right"`, `"Bottom left"`, `"Bottom centre"`, `"Bottom right"`

**Menu:**
- `"Idle"`, `"Recording"`, `"Stopping — saving transcript…"`
- `"Start Recording"`, `"Stop Recording"`, `"Stopping…"`
- `"Transcribe File…"`, `"Open Last Transcript"`, `"Open Sessions Folder"`
- `"Show Overlay"`, `"Keep Overlay Visible"`, `"Unpin Overlay"`
- `"Hide Voi"`, `"Show Voi"`
- `"Settings…"`, `"Audio Permissions…"`, `"Quit Voi"`
- `"Choose a recording to transcribe"`
- Spinner frames `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧`, format `"{f}  {summary}…"`

**Jobs:**
- `"Uploading {name}…"`, `"Uploading"`, `"Queued"`
- `"Transcribing {h:mm:ss} of audio"`, `"Transcribing"`, `"Saving"`

**State (overlay and copy):**
- `"Listening…"`, `"Not recording"`
- Speaker labels `"You"`, `"Them"`

**Errors (stored, not displayed):**
- `"Microphone: {e}"`, `"System audio: {e}"`
- `"no Soniox API key — paste it into Settings, set SONIOX_API_KEY, or write it to {path}"`
- `"reconnecting (attempt {n})"` (warning, log only)

**Command line:**
- `"{tab}: {room} pt to spare"`
- `Off` / `On` / `NeedsApproval` / `Unavailable`
- `"expected on, off or status, not {x:?}"`
- `"only the installed Voi.app can launch at login, not a bare binary"`
- `"this Mac has no Login Items service"`

**Info.plist:**
- `"Voi transcribes your side of a call."`
- `"Voi transcribes what the other participants say."`
- Bundle name `"Voi"`, id `is.mana.voi`

**Version line (log only):** `"Voi {v} · build {b} · {commit}"`, `"Voi {v} (unpackaged)"`

**Log messages (not UI):**
- `"dictation key seen"`, `"dictation starting"`, `"dictation needs a key"`, `"dictation stopping"`, `"dictation cancelled"`
- `"first words"`, `"dictation did not finish in time; using what arrived"`, `"dictation never reached soniox; what was said is lost"`
- `"dictation could not start"`, `"no sound in the bundle"`, `"sound would not load"`
- `"microphone unavailable"`, `"system audio unavailable"`, `"microphone still open with no capture running — a stream was dropped without closing its device"`
- `"could not save the hidden setting"`, `"voi app starting"`, `"settings loaded"`, `"launch at login"`

---

## F. Hard to reproduce on Windows (what the Mac does)

1. **Fn/Globe key.** Read from the `0x800000` modifier flag. Windows normally never sees Fn; it is handled in keyboard firmware.
2. **Left vs right Command and Option via device-dependent flag bits, read by polling with no hook and no permission.** Windows would need `GetAsyncKeyState(VK_RCONTROL/VK_RMENU/VK_RWIN)` polling, or a low-level keyboard hook. The Mac does **not swallow** the key: it still reaches the app. Right Alt on many layouts is AltGr, which is Ctrl+Alt.
3. **Synthetic paste.** `CGEvent` keycode 9 with ⌘, from a private event source posted at the HID tap. The equivalent is `SendInput` Ctrl+V. On Windows that is blocked into elevated (UIPI) windows rather than gated by a permission. The note text says "⌘V".
4. **Accessibility permission flow** (`AXIsProcessTrusted` / `…WithOptions(prompt)`). Without it the text is only copied. Windows has no equivalent gate, so the "Copied" branch would rarely trigger.
5. **TCC preflight.** Opening the mic and the system-audio tap for 250 ms each surfaces the prompts. The usage strings come from Info.plist. The "Audio Permissions…" menu item deep-links to the Privacy pane.
6. **NSSound** from bundle Resources at default volume, stop-then-play restart.
7. **NonactivatingPanel at level 25 (status level), `CanJoinAllSpaces | FullScreenAuxiliary`.** It shows on every Space and over full-screen apps without taking focus. `ignoresMouseEvents` makes clicks pass through while the click-cancel is detected by polling `pressedMouseButtons`.
8. **`NSVisualEffectView`** Popover material, BehindWindow blur, VibrantDark, 13 pt rounded mask. A backdrop blur behind a transparent window; the Windows equivalent would be Acrylic or Mica, or DWM blur.
9. **Core Animation sweep.** A radial `CAGradientLayer` moved by a paced `CAKeyframeAnimation` along a `CGPath`, masked by a painted alpha image.
10. **`visibleFrame`** excludes the menu bar and Dock (Windows: the monitor work area). Coordinates are y-up from the bottom-left of the primary screen.
11. **Status item template image** auto-tinted to the menu bar colour when idle and red when active. Menu ⌘ key equivalents only work inside the open menu.
12. **Accessory activation policy** and `LSUIElement`: no Dock icon.
13. **Login Items via `SMAppService`**, which only works for the installed `.app`.
14. **Mic-in-use detection** via Core Audio process objects (`kAudioProcessPropertyIsRunningInput` by PID), polled every 1.2 s.
15. **Monospaced system font** (SF Mono 13, line height measured 16.0) and the system font for "Cancel". Windows would substitute Cascadia or Consolas and Segoe UI.