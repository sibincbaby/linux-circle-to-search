from circle_search import BTN_H, bbox, button_rects

# padded box around a stroke
assert bbox([(100, 100), (200, 150), (150, 250)], 1920, 1080, pad=8) == (92, 92, 116, 166)
# clamped at screen edges
assert bbox([(2, 3), (1919, 1079)], 1920, 1080, pad=8) == (0, 0, 1920, 1080)
# buttons go below the selection when there's room, above it near the bottom edge
assert button_rects((100, 100, 300, 200), 1920, 1080)[0][1] == 312
assert button_rects((100, 900, 300, 170), 1920, 1080)[0][1] == 900 - 12 - BTN_H
# full-screen selection: buttons sit inside, and never run off the right edge
rects = button_rects((0, 0, 1920, 1080), 1920, 1080)
assert rects[0][1] == 1080 - 12 - BTN_H and rects[-1][0] + rects[-1][2] <= 1920
assert button_rects((1800, 100, 100, 100), 1920, 1080)[-1][0] + 150 <= 1920
print("ok")
